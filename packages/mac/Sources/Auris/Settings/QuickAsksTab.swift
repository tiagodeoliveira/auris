// QuickAsksTab — Settings → Quick Asks. List the user's library +
// add/edit/delete saved chat prompts. Server keeps the canonical
// state; this view reads `model.quickAsks` and writes via
// `upsertQuickAsk` / `deleteQuickAsk`.
//
// Row visual matches `ArtifactRow` (card with rounded border, icon
// + name + meta on top row, secondary text below, trash icon on the
// right). No refresh button — the server pushes the library on
// every change via items_update, so the view is always live.

import Foundation
import SwiftUI

struct QuickAsksTab: View {
    @Bindable var model: AppModel
    @State private var editing: EditingState?

    var body: some View {
        SettingsTabShell(
            title: "Quick Asks",
            description: "Saved prompts you can fire from chat or pick on the glasses.",
            action: {
                Button {
                    let maxPos = model.quickAsks
                        .map { Int32(truncatingIfNeeded: $0.t) }
                        .max() ?? 0
                    editing = EditingState(
                        id: UUID().uuidString,
                        label: "",
                        text: "",
                        position: maxPos + 10,
                        isNew: true
                    )
                } label: {
                    Label("Add", systemImage: "plus")
                }
                .buttonStyle(.borderedProminent)
                .tint(SettingsTheme.blue)
            }
        ) {
            if model.quickAsks.isEmpty {
                VStack(spacing: 8) {
                    Image(systemName: "text.bubble")
                        .font(.largeTitle)
                        .foregroundStyle(.tertiary)
                    Text("No quick asks yet")
                        .foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollView {
                    LazyVStack(spacing: 8) {
                        ForEach(model.quickAsks, id: \.id) { ask in
                            QuickAskRow(
                                ask: ask,
                                onEdit: {
                                    editing = EditingState(
                                        id: ask.id,
                                        label: ask.text,
                                        text: ask.detail ?? "",
                                        position: Int32(truncatingIfNeeded: ask.t),
                                        isNew: false
                                    )
                                },
                                onDelete: {
                                    Task { await model.deleteQuickAsk(id: ask.id) }
                                }
                            )
                        }
                    }
                    .padding(.vertical, 2)
                }
            }
        }
        .sheet(item: $editing) { state in
            QuickAskEditor(state: state, model: model) {
                editing = nil
            }
        }
    }
}

/// Single quick-ask row. Visual mirrors `ArtifactRow`: card with a
/// rounded border, icon on the left, name + preview text stacked,
/// trash on the right. Whole card is tappable to open the editor;
/// trash short-circuits the tap and asks for confirmation.
private struct QuickAskRow: View {
    let ask: Item
    let onEdit: () -> Void
    let onDelete: () -> Void
    @State private var confirmDelete = false

    private var preview: String {
        let raw = (ask.detail ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        return raw.split(separator: "\n").first.map(String.init) ?? raw
    }

    var body: some View {
        Button(action: onEdit) {
            HStack(alignment: .top, spacing: 10) {
                Image(systemName: "text.bubble.fill")
                    .font(.system(size: 14))
                    .foregroundStyle(SettingsTheme.blue)
                    .frame(width: 18, height: 18)
                    .padding(.top, 2)
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 8) {
                        Text(ask.text)
                            .font(.body)
                            .fontWeight(.medium)
                            .lineLimit(1)
                        Spacer(minLength: 4)
                        Button {
                            confirmDelete = true
                        } label: {
                            Image(systemName: "trash")
                                .font(.system(size: 12))
                                .foregroundStyle(.secondary)
                        }
                        .buttonStyle(.plain)
                        .help("Delete quick ask")
                        .confirmationDialog(
                            "Delete “\(ask.text)”?",
                            isPresented: $confirmDelete,
                            titleVisibility: .visible
                        ) {
                            Button("Delete", role: .destructive, action: onDelete)
                            Button("Cancel", role: .cancel) {}
                        } message: {
                            Text("This removes the saved prompt from your library. It cannot be undone.")
                        }
                    }
                    if !preview.isEmpty {
                        Text(preview)
                            .font(.callout)
                            .foregroundStyle(.secondary)
                            .lineLimit(2)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .topLeading)
            }
            .padding(10)
            .background(SettingsTheme.card)
            .clipShape(RoundedRectangle(cornerRadius: 8))
            .overlay(
                RoundedRectangle(cornerRadius: 8)
                    .strokeBorder(SettingsTheme.border)
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .contextMenu {
            Button(role: .destructive, action: onDelete) {
                Label("Delete quick ask", systemImage: "trash")
            }
        }
    }
}

/// Editor sheet. Single source of truth for create + update + delete.
private struct EditingState: Identifiable {
    let id: String
    var label: String
    var text: String
    var position: Int32
    var isNew: Bool
}

private struct QuickAskEditor: View {
    @State var state: EditingState
    @Bindable var model: AppModel
    let onClose: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(state.isNew ? "New Quick Ask" : "Edit Quick Ask")
                .font(.title3.weight(.semibold))

            VStack(alignment: .leading, spacing: 4) {
                Text("Label").font(.caption).foregroundStyle(.secondary)
                TextField("Short mnemonic", text: $state.label)
                    .textFieldStyle(.roundedBorder)
            }

            VStack(alignment: .leading, spacing: 4) {
                Text("Prompt").font(.caption).foregroundStyle(.secondary)
                TextEditor(text: $state.text)
                    .font(.body.monospaced())
                    .frame(minHeight: 200)
                    .overlay(
                        RoundedRectangle(cornerRadius: 6)
                            .strokeBorder(SettingsTheme.border)
                    )
            }

            HStack {
                if !state.isNew {
                    Button(role: .destructive) {
                        Task {
                            await model.deleteQuickAsk(id: state.id)
                            onClose()
                        }
                    } label: {
                        Label("Delete", systemImage: "trash")
                    }
                }
                Spacer()
                Button("Cancel", action: onClose)
                Button {
                    Task {
                        await model.upsertQuickAsk(
                            id: state.id,
                            label: state.label,
                            text: state.text,
                            position: state.position
                        )
                        onClose()
                    }
                } label: {
                    Text("Save")
                }
                .buttonStyle(.borderedProminent)
                .tint(SettingsTheme.blue)
                .disabled(
                    state.label.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ||
                    state.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                )
            }
        }
        .padding(20)
        .frame(width: 480)
    }
}
