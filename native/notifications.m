// SPDX-License-Identifier: MPL-2.0
#import <AppKit/AppKit.h>
#import <UserNotifications/UserNotifications.h>
#include <stdbool.h>

typedef void (*BHNotificationClick)(const char *target);

@interface BHNotifications : NSObject <UNUserNotificationCenterDelegate>
@property(nonatomic) BHNotificationClick onClick;
@property(nonatomic, strong) UNUserNotificationCenter *center;
@end

// UNUserNotificationCenter.delegate is weak. Retain it for the entire process,
// including clicks delivered after a notification relaunches Blackholes.
static BHNotifications *notifications;
static NSString *const BHTargetKey = @"blackholesTarget";

@implementation BHNotifications
- (void)userNotificationCenter:(UNUserNotificationCenter *)center
       willPresentNotification:(UNNotification *)notification
         withCompletionHandler:(void (^)(UNNotificationPresentationOptions))completionHandler {
    // The existing attention sound is played once by the app, not again here.
    completionHandler(UNNotificationPresentationOptionBanner | UNNotificationPresentationOptionList);
}

- (void)userNotificationCenter:(UNUserNotificationCenter *)center
didReceiveNotificationResponse:(UNNotificationResponse *)response
         withCompletionHandler:(void (^)(void))completionHandler {
    if ([response.actionIdentifier isEqualToString:UNNotificationDefaultActionIdentifier]) {
        id value = response.notification.request.content.userInfo[BHTargetKey];
        // An older or malformed notice still brings the app forward. Rust
        // validates the optional navigation destination against live app state.
        NSString *target = [value isKindOfClass:NSString.class] ? value : @"";
        if (self.onClick) self.onClick(target.UTF8String);
        [center removeDeliveredNotificationsWithIdentifiers:@[response.notification.request.identifier]];
    }
    completionHandler();
}
@end

bool bh_notifications_init(BHNotificationClick onClick) {
    NSCAssert(NSThread.isMainThread, @"Initialize notifications on the main thread");
    NSBundle *bundle = NSBundle.mainBundle;
    // UserNotifications uses the actual bundle's identity and icon. A bare
    // development executable has neither; never impersonate Terminal/Finder or
    // swizzle NSBundle (the legacy backend did both).
    if (![bundle.bundleIdentifier isEqualToString:@"dev.blackholes.rust"] ||
        ![bundle.bundleURL.pathExtension isEqualToString:@"app"]) return false;

    if (!notifications) {
        notifications = [BHNotifications new];
        notifications.center = UNUserNotificationCenter.currentNotificationCenter;
    }
    notifications.onClick = onClick;
    notifications.center.delegate = notifications;
    return true;
}

void bh_notifications_activate(void) {
    // Defer until after GPUI finishes handling the click, avoiding reentrant
    // AppKit focus events while GPUI is updating its application state.
    dispatch_async(dispatch_get_main_queue(), ^{
        [NSApp unhide:nil];
        for (NSWindow *window in NSApp.windows) {
            if (!window.canBecomeMainWindow) continue;
            if (window.miniaturized) [window deminiaturize:nil];
            [window makeKeyAndOrderFront:nil];
            break;
        }
        [NSApp activateIgnoringOtherApps:YES];
    });
}

void bh_notifications_show(const char *identifier, const char *title,
                           const char *message, const char *target) {
    @autoreleasepool {
        // Copy Rust-owned strings before returning across the FFI boundary.
        NSString *requestID = [NSString stringWithUTF8String:identifier];
        UNMutableNotificationContent *content = [UNMutableNotificationContent new];
        content.title = [NSString stringWithUTF8String:title];
        content.body = [NSString stringWithUTF8String:message];
        content.userInfo = @{BHTargetKey: [NSString stringWithUTF8String:target]};
        dispatch_async(dispatch_get_main_queue(), ^{
            UNUserNotificationCenter *center = notifications.center;
            if (!center) return;
            UNNotificationRequest *request = [UNNotificationRequest requestWithIdentifier:requestID
                                                                                content:content trigger:nil];
            void (^deliver)(void) = ^{
                [center addNotificationRequest:request withCompletionHandler:^(NSError *error) {
                    if (error) NSLog(@"Blackholes notification delivery failed: %@", error.localizedDescription);
                }];
            };
            [center getNotificationSettingsWithCompletionHandler:^(UNNotificationSettings *settings) {
                if (settings.authorizationStatus == UNAuthorizationStatusNotDetermined) {
                    // Ask in context, on the first real notice, rather than at launch.
                    [center requestAuthorizationWithOptions:UNAuthorizationOptionAlert
                                         completionHandler:^(BOOL granted, NSError *error) {
                        if (error) NSLog(@"Blackholes notification authorization failed: %@", error.localizedDescription);
                        if (granted) deliver();
                    }];
                } else if (settings.authorizationStatus == UNAuthorizationStatusAuthorized ||
                           settings.authorizationStatus == UNAuthorizationStatusProvisional) {
                    deliver();
                }
                // A denial keeps the existing in-app toast without repeated prompts.
            }];
        });
    }
}
