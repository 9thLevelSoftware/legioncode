#pragma once

#include "LegionSourceTextRunShadowNode.h"

#include <react/renderer/componentregistry/ComponentDescriptorProviderRegistry.h>
#include <react/renderer/core/ConcreteComponentDescriptor.h>

namespace facebook::react {
using LegionSourceTextRunComponentDescriptor =
    ConcreteComponentDescriptor<LegionSourceTextRunShadowNode>;

void LegionSourceTextRunSpec_registerComponentDescriptorsFromCodegen(
    std::shared_ptr<const ComponentDescriptorProviderRegistry> registry);
} // namespace facebook::react
