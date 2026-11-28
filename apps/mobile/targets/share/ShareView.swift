import SwiftUI

/// The share sheet's Nexus page: a compact preview of what will be kept, what was left out and why, then
/// Save. System controls and fonts, so Dynamic Type, VoiceOver and Dark Mode work as in any iOS sheet.
struct ShareView: View {
  @ObservedObject var model: ShareModel

  var body: some View {
    NavigationStack {
      Group {
        switch model.phase {
        case .loading:
          ProgressView("Reading what you shared…")
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .unavailable:
          message(title: "Nexus can't keep this", detail: "This build of Nexus can't keep shared items on your iPhone.", symbol: "exclamationmark.triangle")
        case .done(let outcome):
          message(title: ShareWords.title(outcome), detail: ShareWords.detail(outcome, signedIn: model.signedIn), symbol: outcome == .saved || outcome == .alreadySaved ? "checkmark.circle" : "exclamationmark.circle")
        case .ready, .saving:
          preview
        }
      }
      .navigationTitle("Nexus")
      .navigationBarTitleDisplayMode(.inline)
      .toolbar { toolbar }
    }
  }

  @ToolbarContentBuilder private var toolbar: some ToolbarContent {
    switch model.phase {
    case .done, .unavailable:
      ToolbarItem(placement: .confirmationAction) { Button("Done") { model.done() } }
    default:
      ToolbarItem(placement: .cancellationAction) { Button("Cancel") { model.cancel() } }
      ToolbarItem(placement: .confirmationAction) {
        if model.phase == .saving {
          ProgressView().accessibilityLabel("Saving")
        } else {
          Button("Save") { model.save() }.disabled(!model.canSave)
        }
      }
    }
  }

  private var preview: some View {
    let content = model.content
    return Form {
      if content.isEmpty {
        Section {
          Text("Nexus can't use anything in this share.")
        }
      }
      if !content.text.isEmpty {
        Section {
          Text(content.text)
            .lineLimit(8)
            .textSelection(.enabled)
        } header: {
          Text("Text")
        } footer: {
          if content.textCut { Text("Only the first \(ShareInboxLimits.textCharacters.formatted()) characters are kept.") }
        }
      }
      if !content.urls.isEmpty {
        Section {
          ForEach(content.urls, id: \.self) { link in
            VStack(alignment: .leading, spacing: 2) {
              Text(URL(string: link)?.host() ?? link).font(.headline)
              Text(link).font(.footnote).foregroundStyle(.secondary).lineLimit(2)
            }
            .accessibilityElement(children: .combine)
          }
        } header: {
          Text(content.urls.count == 1 ? "Link" : "Links")
        } footer: {
          VStack(alignment: .leading, spacing: 4) {
            if content.linksCut { Text("Only the first \(ShareInboxLimits.urls) links are kept.") }
            Text("Nexus gets the address only. It can't open a page that needs your sign-in.")
          }
        }
      }
      if !content.images.isEmpty {
        Section(content.images.count == 1 ? "Photo" : "\(content.images.count) photos") {
          LazyVGrid(columns: [GridItem(.adaptive(minimum: 76), spacing: 8)], spacing: 8) {
            ForEach(content.images) { image in
              thumbnail(image)
            }
          }
          .padding(.vertical, 4)
        }
      }
      if !content.skipped.isEmpty {
        Section("Not saved") {
          ForEach(content.skipped) { item in
            VStack(alignment: .leading, spacing: 2) {
              Text(item.name).lineLimit(2)
              Text(ShareWords.reason(item.reason)).font(.footnote).foregroundStyle(.secondary)
            }
            .accessibilityElement(children: .combine)
          }
        }
      }
      if !content.isEmpty {
        Section {
          EmptyView()
        } footer: {
          Text(model.signedIn
            ? "Save keeps this on your iPhone. Then open Nexus to choose a chat or the Assistant. Nothing is sent until you tap Send."
            : "Save keeps this on your iPhone. Sign in to Nexus to use it. Nothing is sent until you tap Send.")
        }
      }
    }
  }

  private func thumbnail(_ image: StagedImage) -> some View {
    Group {
      if let picture = model.thumbnails[image.id] {
        Image(uiImage: picture).resizable().scaledToFill()
      } else {
        Image(systemName: "photo").foregroundStyle(.secondary)
      }
    }
    .frame(width: 76, height: 76)
    .background(Color(.secondarySystemBackground))
    .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
    .accessibilityElement()
    .accessibilityLabel(image.name)
    .accessibilityAddTraits(.isImage)
  }

  private func message(title: String, detail: String, symbol: String) -> some View {
    ScrollView {
      VStack(spacing: 12) {
        Image(systemName: symbol)
          .font(.largeTitle)
          .foregroundStyle(.secondary)
          .accessibilityHidden(true)
        Text(title).font(.title3.weight(.semibold)).multilineTextAlignment(.center)
        Text(detail).font(.body).foregroundStyle(.secondary).multilineTextAlignment(.center)
      }
      .padding(24)
      .frame(maxWidth: .infinity)
      .accessibilityElement(children: .combine)
    }
  }
}
