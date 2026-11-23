import ExpoModulesCore

public class LegionTerminalModule: Module {
  public func definition() -> ModuleDefinition {
    Name("LegionTerminalSurface")

    // Bumped when native hardware-keyboard handling changes; surfaced in the JS debug
    // logs so a stale native binary is distinguishable from a broken key pipeline.
    Constants([
      "hardwareKeyRevision": 4,
      "outputRevision": 1,
    ])

    View(LegionTerminalView.self) {
      AsyncFunction("writeOutput") { (view: LegionTerminalView, data: String, reset: Bool) in
        view.writeOutput(data, reset: reset)
      }

      AsyncFunction("resetInput") { (view: LegionTerminalView) in
        view.resetInputField()
      }

      Prop("terminalKey") { (view: LegionTerminalView, terminalKey: String) in
        view.terminalKey = terminalKey
      }

      Prop("initialBuffer") { (view: LegionTerminalView, initialBuffer: String) in
        view.initialBuffer = initialBuffer
      }

      Prop("fontSize") { (view: LegionTerminalView, fontSize: Double) in
        view.fontSize = CGFloat(fontSize)
      }

      Prop("focusRequest") { (view: LegionTerminalView, focusRequest: Double) in
        view.focusRequest = focusRequest
      }

      Prop("readOnly") { (view: LegionTerminalView, readOnly: Bool) in
        view.readOnly = readOnly
      }

      Prop("autoFocus") { (view: LegionTerminalView, autoFocus: Bool) in
        view.autoFocus = autoFocus
      }

      Prop("appearanceScheme") { (view: LegionTerminalView, appearanceScheme: String) in
        view.appearanceScheme = appearanceScheme
      }

      Prop("themeConfig") { (view: LegionTerminalView, themeConfig: String) in
        view.themeConfig = themeConfig
      }

      Prop("backgroundColor") { (view: LegionTerminalView, backgroundColor: String) in
        view.backgroundColorHex = backgroundColor
      }

      Prop("foregroundColor") { (view: LegionTerminalView, foregroundColor: String) in
        view.foregroundColorHex = foregroundColor
      }

      Prop("mutedForegroundColor") { (view: LegionTerminalView, mutedForegroundColor: String) in
        view.mutedForegroundColorHex = mutedForegroundColor
      }

      Prop("captureRequest") { (view: LegionTerminalView, request: Double) in
        view.captureRequest = request
      }

      Prop("sharedGrid") { (view: LegionTerminalView, sharedGrid: [Int]) in
        view.sharedGrid = sharedGrid
      }
      Events("onInput", "onResize", "onCapture", "onPaste")

      OnViewDidUpdateProps { (view: LegionTerminalView) in
        view.applyPendingBuffer()
      }
    }
  }
}
