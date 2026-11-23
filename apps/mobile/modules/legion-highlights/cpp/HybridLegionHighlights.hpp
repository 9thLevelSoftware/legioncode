#pragma once

#include "HybridLegionHighlightsSpec.hpp"

namespace margelo::nitro::legionhighlights {

class HybridLegionHighlights : public HybridLegionHighlightsSpec {
public:
  HybridLegionHighlights() : HybridObject(TAG) {}

  std::optional<std::shared_ptr<ArrayBuffer>> tokenize(SourceText source,
                                                       double language) override;
};

} // namespace margelo::nitro::legionhighlights
