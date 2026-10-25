// Mush8 - route.rs
// Constantin Clerc (c22dev) - 0.1
pub use platform::Watcher;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Route {
    Headphones,
    Speakers,
    External,
}

pub type Notify = Box<dyn Fn() + Send + Sync>;

#[cfg(target_os = "macos")]
mod platform {
    use super::{Notify, Route};
    use objc2_core_audio::{
        AudioObjectAddPropertyListener, AudioObjectGetPropertyData, AudioObjectID, AudioObjectPropertyAddress, AudioObjectPropertySelector, AudioObjectRemovePropertyListener,
        kAudioDevicePropertyBufferFrameSize, kAudioDevicePropertyDataSource, kAudioDevicePropertyLatency, kAudioDevicePropertyNominalSampleRate, kAudioDevicePropertySafetyOffset,
        kAudioDevicePropertyStreams, kAudioDevicePropertyTransportType, kAudioStreamPropertyLatency, kAudioDeviceTransportTypeBluetooth, kAudioDeviceTransportTypeBluetoothLE, kAudioDeviceTransportTypeBuiltIn,
        kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyScopeOutput, kAudioObjectSystemObject,
    };
    use std::ffi::c_void;
    use std::ptr::NonNull;

    const SYSTEM: AudioObjectID = kAudioObjectSystemObject as AudioObjectID;
    const HEADPHONE_JACK: u32 = u32::from_be_bytes(*b"hdpn");
    const BLUETOOTH: u32 = kAudioDeviceTransportTypeBluetooth;
    const BLUETOOTH_LE: u32 = kAudioDeviceTransportTypeBluetoothLE;
    const BUILT_IN: u32 = kAudioDeviceTransportTypeBuiltIn;
    const DEFAULT_OUTPUT: AudioObjectPropertyAddress = address(kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyScopeGlobal);
    const DATA_SOURCE: AudioObjectPropertyAddress = address(kAudioDevicePropertyDataSource, kAudioObjectPropertyScopeOutput);
    const TRANSPORT: AudioObjectPropertyAddress = address(kAudioDevicePropertyTransportType, kAudioObjectPropertyScopeGlobal);
    const NOMINAL_RATE: AudioObjectPropertyAddress = address(kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal);
    const STREAMS: AudioObjectPropertyAddress = address(kAudioDevicePropertyStreams, kAudioObjectPropertyScopeOutput);
    const DEVICE_LATENCY: AudioObjectPropertyAddress = address(kAudioDevicePropertyLatency, kAudioObjectPropertyScopeOutput);
    const SAFETY_OFFSET: AudioObjectPropertyAddress = address(kAudioDevicePropertySafetyOffset, kAudioObjectPropertyScopeOutput);
    const BUFFER_FRAMES: AudioObjectPropertyAddress = address(kAudioDevicePropertyBufferFrameSize, kAudioObjectPropertyScopeGlobal);
    const STREAM_LATENCY: AudioObjectPropertyAddress = address(kAudioStreamPropertyLatency, kAudioObjectPropertyScopeGlobal);

    const fn address(selector: AudioObjectPropertySelector, scope: u32) -> AudioObjectPropertyAddress {
        AudioObjectPropertyAddress { mSelector: selector, mScope: scope, mElement: kAudioObjectPropertyElementMain }
    }

    fn get<T: Default>(object: AudioObjectID, addr: AudioObjectPropertyAddress) -> Option<T> {
        let (mut value, mut size) = (T::default(), size_of::<T>() as u32);
        let status = unsafe { AudioObjectGetPropertyData(object, NonNull::from(&addr), 0, std::ptr::null(), NonNull::from(&mut size), NonNull::from(&mut value).cast()) };
        (status == 0).then_some(value)
    }

    unsafe extern "C-unwind" fn changed(_: AudioObjectID, _: u32, _: NonNull<AudioObjectPropertyAddress>, notify: *mut c_void) -> i32 {
        (unsafe { &*(notify as *const Notify) })();
        0
    }

    pub struct Watcher {
        notify: Box<Notify>,
        device: AudioObjectID,
    }

    impl Watcher {
        pub fn new(notify: Notify) -> Self {
            let w = Watcher { notify: Box::new(notify), device: 0 };
            w.listen(SYSTEM, DEFAULT_OUTPUT, true);
            w
        }

        fn listen(&self, object: AudioObjectID, addr: AudioObjectPropertyAddress, on: bool) {
            let ctx = &*self.notify as *const Notify as *mut c_void;
            unsafe {
                if on {
                    AudioObjectAddPropertyListener(object, NonNull::from(&addr), Some(changed), ctx);
                } else {
                    AudioObjectRemovePropertyListener(object, NonNull::from(&addr), Some(changed), ctx);
                }
            }
        }

        fn follow(&mut self, device: AudioObjectID) {
            if device == self.device {
                return;
            }
            if self.device != 0 {
                self.listen(self.device, DATA_SOURCE, false);
            }
            if device != 0 {
                self.listen(device, DATA_SOURCE, true);
            }
            self.device = device;
        }

        pub fn route(&mut self) -> Route {
            let device = get(SYSTEM, DEFAULT_OUTPUT).unwrap_or(0);
            self.follow(device);
            match get::<u32>(device, TRANSPORT) {
                Some(BLUETOOTH | BLUETOOTH_LE) => Route::Headphones,
                Some(BUILT_IN) if get(device, DATA_SOURCE) == Some(HEADPHONE_JACK) => Route::Headphones,
                Some(BUILT_IN) => Route::Speakers,
                _ => Route::External,
            }
        }

        pub fn latency(&self) -> f64 {
            let d = self.device;
            let rate = get::<f64>(d, NOMINAL_RATE).filter(|&r| r > 0.0).unwrap_or(48000.0);
            let stream = get::<u32>(d, STREAMS).unwrap_or(0);
            let frames: u32 = [get::<u32>(d, DEVICE_LATENCY), get(d, SAFETY_OFFSET), get(d, BUFFER_FRAMES), get(stream, STREAM_LATENCY)].into_iter().flatten().sum();
            frames as f64 / rate
        }
    }

    impl Drop for Watcher {
        fn drop(&mut self) {
            self.follow(0);
            self.listen(SYSTEM, DEFAULT_OUTPUT, false);
        }
    }
}

#[cfg(target_os = "ios")]
mod platform {
    use super::{Notify, Route};
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_avf_audio::{
        AVAudioSession, AVAudioSessionPort, AVAudioSessionPortBluetoothA2DP, AVAudioSessionPortBluetoothHFP, AVAudioSessionPortBluetoothLE, AVAudioSessionPortBuiltInReceiver,
        AVAudioSessionPortBuiltInSpeaker, AVAudioSessionPortHeadphones, AVAudioSessionRouteChangeNotification,
    };
    use objc2_foundation::{NSNotification, NSNotificationCenter, NSObjectProtocol};
    use std::ptr::NonNull;

    pub struct Watcher {
        token: Retained<ProtocolObject<dyn NSObjectProtocol>>,
    }

    impl Watcher {
        pub fn new(notify: Notify) -> Self {
            let block = RcBlock::new(move |_: NonNull<NSNotification>| notify());
            let token = unsafe { NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(AVAudioSessionRouteChangeNotification, None, None, &block) };
            Watcher { token }
        }

        pub fn route(&mut self) -> Route {
            let Some(port) = (unsafe { AVAudioSession::sharedInstance().currentRoute().outputs() }).firstObject() else { return Route::Speakers };
            let kind = unsafe { port.portType() };
            let any = |ports: &[Option<&AVAudioSessionPort>]| ports.iter().flatten().any(|p| **p == *kind);
            unsafe {
                if any(&[AVAudioSessionPortHeadphones, AVAudioSessionPortBluetoothA2DP, AVAudioSessionPortBluetoothLE, AVAudioSessionPortBluetoothHFP]) {
                    Route::Headphones
                } else if any(&[AVAudioSessionPortBuiltInSpeaker, AVAudioSessionPortBuiltInReceiver]) {
                    Route::Speakers
                } else {
                    Route::External
                }
            }
        }

        pub fn latency(&self) -> f64 {
            let session = unsafe { AVAudioSession::sharedInstance() };
            unsafe { session.outputLatency() + session.IOBufferDuration() }
        }
    }

    impl Drop for Watcher {
        fn drop(&mut self) {
            unsafe { NSNotificationCenter::defaultCenter().removeObserver(self.token.as_ref()) };
        }
    }
}
