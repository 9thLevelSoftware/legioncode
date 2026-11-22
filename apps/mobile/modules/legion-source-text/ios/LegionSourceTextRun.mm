#import "LegionSourceTextRun.h"
#import <React/RCTFabricComponentsPlugins.h>
#import "LegionSourceTextRunComponentDescriptor.h"
#import <react/renderer/components/LegionSourceTextSpec/Props.h>
#import <react/renderer/components/LegionSourceTextSpec/RCTComponentViewHelpers.h>

using namespace facebook::react;

@interface LegionSourceTextRun () <RCTLegionSourceTextRunViewProtocol>

@end

@implementation LegionSourceTextRun {
  NSString *_text;
}

+ (ComponentDescriptorProvider)componentDescriptorProvider {
  return concreteComponentDescriptorProvider<
      LegionSourceTextRunComponentDescriptor>();
}

- (instancetype)initWithFrame:(CGRect)frame {
  if (self = [super initWithFrame:frame]) {
    static const auto defaultProps =
        std::make_shared<const LegionSourceTextRunProps>();
    _props = defaultProps;
  }
  return self;
}

- (void)updateProps:(Props::Shared const &)props
           oldProps:(Props::Shared const &)oldProps {
  const auto &oldViewProps =
      *std::static_pointer_cast<LegionSourceTextRunProps const>(_props);
  const auto &newViewProps =
      *std::static_pointer_cast<LegionSourceTextRunProps const>(props);

  if (newViewProps.text != oldViewProps.text) {
    NSString *text = [NSString stringWithUTF8String:newViewProps.text.c_str()];
    _text = text;
  }

  [super updateProps:props oldProps:oldProps];
}

+ (BOOL)shouldBeRecycled {
  return NO;
}

Class<RCTComponentViewProtocol> LegionSourceTextRunCls(void) {
  return LegionSourceTextRun.class;
}

@end
