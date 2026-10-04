// SPDX-License-Identifier: MPL-2.0
#import <AppKit/AppKit.h>
#import <WebKit/WebKit.h>
#import <objc/runtime.h>
#include <stdint.h>

// Intercept arrows before AppKit's text input handling or either responder
// tree. The monitor is active only for this WebView's quick-open dialog.
@interface BHQuickOpenKeyboard : NSObject
@property(nonatomic, weak) WKWebView *webview;
@property(nonatomic) uint64_t openID;
@property(nonatomic, strong) id monitor;
- (void)setDialogID:(uint64_t)openID;
@end

@implementation BHQuickOpenKeyboard
- (void)setDialogID:(uint64_t)openID {
    self.openID = openID;
    if (!openID) {
        if (self.monitor) [NSEvent removeMonitor:self.monitor];
        self.monitor = nil;
        return;
    }
    if (self.monitor) return;

    __weak BHQuickOpenKeyboard *weakSelf = self;
    self.monitor = [NSEvent addLocalMonitorForEventsMatchingMask:NSEventMaskKeyDown
                                                      handler:^NSEvent *(NSEvent *event) {
        BHQuickOpenKeyboard *keyboard = weakSelf;
        WKWebView *webview = keyboard.webview;
        if (!keyboard.openID || !webview.window || event.window != webview.window ||
            !webview.window.isKeyWindow || webview.isHiddenOrHasHiddenAncestor) return event;

        NSString *key;
        switch (event.keyCode) {
            case 123: key = @"ArrowLeft"; break;
            case 124: key = @"ArrowRight"; break;
            case 125: key = @"ArrowDown"; break;
            case 126: key = @"ArrowUp"; break;
            default: return event;
        }
        // Let an active input method use arrows to select its composition.
        NSResponder *responder = event.window.firstResponder;
        if ([responder respondsToSelector:@selector(hasMarkedText)] &&
            [(id<NSTextInputClient>)responder hasMarkedText]) return event;

        NSEventModifierFlags flags = event.modifierFlags;
        NSDictionary *payload = @{
            @"type": @"quick_open_key", @"open_id": @(keyboard.openID), @"key": key,
            @"command": @((flags & (NSEventModifierFlagCommand | NSEventModifierFlagControl)) != 0),
            @"shift": @((flags & NSEventModifierFlagShift) != 0),
            @"alt": @((flags & NSEventModifierFlagOption) != 0)
        };
        NSData *json = [NSJSONSerialization dataWithJSONObject:payload options:0 error:NULL];
        if (!json) return event;
        NSString *serialized = [[NSString alloc] initWithData:json encoding:NSUTF8StringEncoding];
        NSString *script = [NSString stringWithFormat:@"window.blackholesNative?.receive(%@);", serialized];
        [webview evaluateJavaScript:script completionHandler:nil];
        // Consume the original event so neither GPUI nor WKWebView inserts
        // a function character or navigates a second time.
        return nil;
    }];
}

- (void)dealloc {
    if (_monitor) [NSEvent removeMonitor:_monitor];
}
@end

static char BHQuickOpenKeyboardKey;

void bh_quick_open_keyboard_set_dialog(void *view, uint64_t openID) {
    NSCAssert(NSThread.isMainThread, @"Configure quick-open keyboard on the main thread");
    WKWebView *webview = (__bridge WKWebView *)view;
    if (!webview) return;
    BHQuickOpenKeyboard *keyboard = objc_getAssociatedObject(webview, &BHQuickOpenKeyboardKey);
    if (!keyboard && openID) {
        keyboard = [BHQuickOpenKeyboard new];
        keyboard.webview = webview;
        objc_setAssociatedObject(webview, &BHQuickOpenKeyboardKey, keyboard, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    }
    [keyboard setDialogID:openID];
}
