#pragma once

#include <react/renderer/components/LegionSourceTextSpec/EventEmitters.h>
#include <react/renderer/components/LegionSourceTextSpec/Props.h>
#include <react/renderer/components/LegionSourceTextSpec/States.h>
#include <react/renderer/components/view/ConcreteViewShadowNode.h>

namespace facebook::react {
extern const char LegionSourceTextRunComponentName[];

using LegionSourceTextRunShadowNode =
    ConcreteViewShadowNode<LegionSourceTextRunComponentName, LegionSourceTextRunProps,
                           LegionSourceTextRunEventEmitter, LegionSourceTextRunState>;
} // namespace facebook::react
