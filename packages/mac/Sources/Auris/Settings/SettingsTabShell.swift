// SettingsTabShell.swift
// Shared page chrome for every tab in the Settings window. Gives
// each tab the same visual identity: H1 title (matches the tab
// name), one-line description below, optional top-right action
// slot, divider, then the tab-specific body. Without this shell
// every tab grew its own header treatment (or none at all) and
// the window felt like five unrelated screens.
//
// Not used for MeetingsTab — that tab is intrinsically a master/
// detail browser (sidebar + detail) and forcing a top-of-window
// header band would eat vertical space the meeting list needs.
// The other four tabs (Account / Artifacts / Quick Asks /
// Permissions) all share the same shape: header + scrollable body.

import SwiftUI

/// Generic shell so callers can supply any view for the action slot
/// (a Button, a small HStack of buttons, a ProgressView, etc.).
/// `Action == EmptyView` is the convenience case for tabs with no
/// page-level primary action — see the extension below.
struct SettingsTabShell<Action: View, Content: View>: View {
    let title: String
    let description: String?
    @ViewBuilder let action: () -> Action
    @ViewBuilder let content: () -> Content

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                VStack(alignment: .leading, spacing: 6) {
                    Text(title)
                        .font(.title2.weight(.semibold))
                    if let description {
                        Text(description)
                            .font(.subheadline)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                Spacer(minLength: 12)
                action()
            }
            .padding(.bottom, 14)

            Divider()
                .padding(.bottom, 14)

            content()
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        }
        .padding(24)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(SettingsTheme.background)
    }
}

/// No-action convenience: omit the `action` closure entirely when
/// the tab has no page-level primary action (e.g. Permissions).
extension SettingsTabShell where Action == EmptyView {
    init(
        title: String,
        description: String? = nil,
        @ViewBuilder content: @escaping () -> Content
    ) {
        self.init(title: title, description: description, action: { EmptyView() }, content: content)
    }
}
