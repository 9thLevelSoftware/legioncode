package com.margelo.nitro.legionhighlights

import com.facebook.react.BaseReactPackage
import com.facebook.react.bridge.NativeModule
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.module.model.ReactModuleInfoProvider

// Autolinking instantiates this package, which loads the library that registers LegionHighlights.
class LegionHighlightsPackage : BaseReactPackage() {
  override fun getModule(name: String, reactContext: ReactApplicationContext): NativeModule? = null

  override fun getReactModuleInfoProvider() = ReactModuleInfoProvider { emptyMap() }

  companion object {
    init {
      legionhighlightsOnLoad.initializeNative()
    }
  }
}
