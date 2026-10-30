// Mush8 - ModList.swift
// Constantin Clerc (c22dev) - 0.1
import SwiftUI

struct ModRow: View {
    let mod: Mod
    @Binding var disabled: [String]
    var warning: String?
    let inspect: () -> Void
    let delete: () -> Void

    var body: some View {
        Toggle(isOn: Binding(get: { !disabled.contains(mod.id) }, set: { on in
            disabled.removeAll { $0 == mod.id }
            if !on { disabled.append(mod.id) }
        })) {
            VStack(alignment: .leading, spacing: 3) {
                Text(mod.displayTitle).font(.body.weight(.medium)).lineLimit(2)
                Text(mod.summary).font(.caption).foregroundStyle(.secondary)
                if let warning {
                    Text(warning).font(.caption).foregroundStyle(.orange)
                }
                if !canDeleteMod(mod) {
                    Text("Shared folder · view files only").font(.caption).foregroundStyle(.secondary)
                }
                #if os(iOS)
                if !mod.patches.isEmpty {
                    Text("Code patches don't run on iOS").font(.caption).foregroundStyle(.orange)
                }
                #endif
            }
        }
        .accessibilityLabel(mod.name)
        #if os(iOS)
        .accessibilityHint(mod.summary + ". Swipe or long press for actions.")
        #else
        .accessibilityHint(mod.summary + ". Open the context menu to view files or delete.")
        #endif
        .accessibilityAction(named: Text("View files"), inspect)
        .contextMenu {
            Button("View files", systemImage: "folder", action: inspect)
            Button("Delete", systemImage: "trash", role: .destructive, action: delete)
                .disabled(!canDeleteMod(mod))
        }
        .swipeActions(edge: .trailing, allowsFullSwipe: false) {
            Button(action: inspect) { Label("Files", systemImage: "folder") }
                .tint(.blue)
            Button(role: .destructive, action: delete) { Label("Delete", systemImage: "trash") }
                .disabled(!canDeleteMod(mod))
        }
    }
}

struct ModsHeader: View {
    let mods: [Mod]
    let disabled: [String]

    var body: some View {
        HStack {
            Text("Mods")
            if !mods.isEmpty {
                Spacer()
                Text("\(mods.filter { !disabled.contains($0.id) }.count)/\(mods.count) active")
                    .font(.caption).foregroundStyle(.secondary).textCase(nil)
            }
        }
    }
}

extension View {
    func modActions(folder: URL, inspected: Binding<Mod?>, deleting: Binding<Mod?>, error: Binding<String?>,
                    delete: @escaping (Mod) -> Void) -> some View {
        sheet(item: inspected) { mod in ModContentsView(mod: mod, folder: folder) }
            .confirmationDialog("Delete mod?", isPresented: Binding(get: { deleting.wrappedValue != nil }, set: { if !$0 { deleting.wrappedValue = nil } }),
                                presenting: deleting.wrappedValue) { mod in
                Button("Delete", role: .destructive) { delete(mod) }
                Button("Cancel", role: .cancel) { deleting.wrappedValue = nil }
            } message: { mod in
                Text("Permanently delete \(mod.name)? Only \(mod.path) is removed.")
            }
            .alert("Mod action failed", isPresented: Binding(get: { error.wrappedValue != nil }, set: { if !$0 { error.wrappedValue = nil } })) {
                Button("OK") { error.wrappedValue = nil }
            } message: { Text(error.wrappedValue ?? "") }
    }
}
