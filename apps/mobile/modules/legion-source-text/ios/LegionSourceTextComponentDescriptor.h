#pragma once

#include "LegionSourceTextShadowNode.h"

#include <react/renderer/componentregistry/ComponentDescriptorProviderRegistry.h>
#include <react/renderer/core/ConcreteComponentDescriptor.h>

namespace facebook::react {
using LegionSourceTextComponentDescriptor =
    ConcreteComponentDescriptor<LegionSourceTextShadowNode>;

void LegionSourceTextSpec_registerComponentDescriptorsFromCodegen(
    std::shared_ptr<const ComponentDescriptorProviderRegistry> registry);
} // namespace facebook::react
