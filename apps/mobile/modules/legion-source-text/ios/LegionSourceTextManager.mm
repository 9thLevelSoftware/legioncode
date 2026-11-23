#import <React/RCTUIManager.h>
#import <React/RCTViewManager.h>

@interface LegionSourceTextManager : RCTViewManager
@end

@implementation LegionSourceTextManager

RCT_EXPORT_MODULE(LegionSourceText)

- (UIView *)view {
  return [[UIView alloc] init];
}

@end

@interface LegionSourceTextRunManager : RCTViewManager
@end

@implementation LegionSourceTextRunManager

RCT_EXPORT_MODULE(LegionSourceTextRun)

- (UIView *)view {
  return nil;
}

@end
