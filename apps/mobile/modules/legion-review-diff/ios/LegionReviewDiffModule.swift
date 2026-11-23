import ExpoModulesCore

public class LegionReviewDiffModule: Module {
  public func definition() -> ModuleDefinition {
    Name("LegionReviewDiffSurface")

    View(LegionReviewDiffView.self) {
      Prop("contentInsetTop") { (view: LegionReviewDiffView, value: Double) in
        view.setContentInsetTop(CGFloat(value))
      }

      Prop("contentInsetBottom") { (view: LegionReviewDiffView, value: Double) in
        view.setContentInsetBottom(CGFloat(value))
      }

      Prop("tokensResetKey") { (view: LegionReviewDiffView, tokensResetKey: String) in
        view.setTokensResetKey(tokensResetKey)
      }

      Prop("contentResetKey") { (view: LegionReviewDiffView, contentResetKey: String) in
        view.setContentResetKey(contentResetKey)
      }

      Prop("collapsedFileIdsJson") { (view: LegionReviewDiffView, collapsedFileIdsJson: String) in
        view.setCollapsedFileIdsJson(collapsedFileIdsJson)
      }

      Prop("viewedFileIdsJson") { (view: LegionReviewDiffView, viewedFileIdsJson: String) in
        view.setViewedFileIdsJson(viewedFileIdsJson)
      }

      Prop("selectedRowIdsJson") { (view: LegionReviewDiffView, selectedRowIdsJson: String) in
        view.setSelectedRowIdsJson(selectedRowIdsJson)
      }

      Prop("collapsedCommentIdsJson") { (view: LegionReviewDiffView, collapsedCommentIdsJson: String) in
        view.setCollapsedCommentIdsJson(collapsedCommentIdsJson)
      }

      Prop("appearanceScheme") { (view: LegionReviewDiffView, appearanceScheme: String) in
        view.setAppearanceScheme(appearanceScheme)
      }

      Prop("themeJson") { (view: LegionReviewDiffView, themeJson: String) in
        view.setThemeJson(themeJson)
      }

      Prop("styleJson") { (view: LegionReviewDiffView, styleJson: String) in
        view.setStyleJson(styleJson)
      }

      Prop("rowHeight") { (view: LegionReviewDiffView, rowHeight: Double) in
        view.setRowHeight(CGFloat(rowHeight))
      }

      Prop("contentWidth") { (view: LegionReviewDiffView, contentWidth: Double) in
        view.setContentWidth(CGFloat(contentWidth))
      }

      Prop("initialRowIndex") { (view: LegionReviewDiffView, initialRowIndex: Double) in
        view.setInitialRowIndex(initialRowIndex)
      }

      Prop("initialFileId") { (view: LegionReviewDiffView, fileId: String) in
        view.setInitialFileId(fileId)
      }

      Prop("scrollEnabled") { (view: LegionReviewDiffView, enabled: Bool) in
        view.setScrollEnabled(enabled)
      }

      Prop("bounces") { (view: LegionReviewDiffView, bounces: Bool) in
        view.setBounces(bounces)
      }

      Prop("refreshEnabled") { (view: LegionReviewDiffView, enabled: Bool) in
        view.setRefreshEnabled(enabled)
      }

      Prop("refreshing") { (view: LegionReviewDiffView, refreshing: Bool) in
        view.setRefreshing(refreshing)
      }

      Events(
        "onDebug",
        "onVisibleFileChange",
        "onToggleFile",
        "onToggleViewedFile",
        "onPressLine",
        "onToggleComment",
        "onPressNotice",
        "onPullToRefresh"
      )

      AsyncFunction("scrollToFile") { (view: LegionReviewDiffView, fileId: String, animated: Bool) in
        view.scrollToFile(fileId, animated: animated)
      }

      AsyncFunction("scrollToTop") { (view: LegionReviewDiffView, animated: Bool) in
        view.scrollToTop(animated: animated)
      }

      // Large, frequently changing JSON values cannot be regular Fabric props. Expo's
      // prop adapter compares strings on the main thread before invoking a setter, which
      // makes a syntax-token patch capable of blocking a frame by itself.
      AsyncFunction("setRowsJson") { (view: LegionReviewDiffView, rowsJson: String, contentResetKey: String) in
        view.setContentResetKey(contentResetKey)
        view.setRowsJson(rowsJson)
      }

      AsyncFunction("setTokensJson") { (view: LegionReviewDiffView, tokensJson: String) in
        view.setTokensJson(tokensJson)
      }

      AsyncFunction("setTokensPatchJson") { (view: LegionReviewDiffView, tokensPatchJson: String, promise: Promise) in
        view.setTokensPatchJson(tokensPatchJson) { accepted in
          promise.resolve(accepted)
        }
      }
    }
  }
}
