// SPDX-License-Identifier: MPL-2.0
#import <AppKit/AppKit.h>
#import <Sparkle/Sparkle.h>
#include <stdbool.h>

// Sparkle is loaded only from the signed app bundle, never from PATH or a URL.
// Keeping the reference alive also keeps its delegate alive for the app lifetime.
@interface BHUpdater : NSObject <SPUUpdaterDelegate, SPUStandardUserDriverDelegate>
@property(nonatomic, strong) SPUStandardUpdaterController *controller;
@property(nonatomic, copy) NSString *version;
@property(nonatomic, copy) NSString *errorMessage;
@property(nonatomic, copy) void (^pendingInstall)(void);
@property(nonatomic) BOOL blocked;
@property(nonatomic) BOOL downloading;
@property(nonatomic) BOOL spanish;
- (void)requestRestart;
@end

@implementation BHUpdater
- (BOOL)supportsGentleScheduledUpdateReminders { return YES; }
- (BOOL)standardUserDriverShouldHandleShowingScheduledUpdate:(SUAppcastItem *)item andInImmediateFocus:(BOOL)focus { return NO; }
- (void)standardUserDriverWillHandleShowingUpdate:(BOOL)handle forUpdate:(SUAppcastItem *)item state:(SPUUserUpdateState *)state {
    self.version = item.displayVersionString;
}
- (void)updater:(SPUUpdater *)updater didFindValidUpdate:(SUAppcastItem *)item {
    self.errorMessage = nil;
    self.version = item.displayVersionString;
}
- (void)updaterDidNotFindUpdate:(SPUUpdater *)updater {
    self.version = nil;
    self.errorMessage = nil;
}
- (void)updater:(SPUUpdater *)updater didAbortWithError:(NSError *)error {
    self.errorMessage = error.localizedDescription;
    self.downloading = NO;
}
- (void)updater:(SPUUpdater *)updater willDownloadUpdate:(SUAppcastItem *)item withRequest:(NSMutableURLRequest *)request { self.downloading = YES; }
- (void)userDidCancelDownload:(SPUUpdater *)updater { self.downloading = NO; }
- (void)standardUserDriverWillFinishUpdateSession {
    if (!self.pendingInstall) { self.version = nil; self.downloading = NO; }
}
- (BOOL)updater:(SPUUpdater *)updater shouldPostponeRelaunchForUpdate:(SUAppcastItem *)item untilInvokingBlock:(void (^)(void))handler {
    self.pendingInstall = handler;
    self.downloading = NO;
    dispatch_async(dispatch_get_main_queue(), ^{ [self requestRestart]; });
    return YES;
}
- (void)requestRestart {
    if (!self.pendingInstall) return;
    NSAlert *alert = [NSAlert new];
    if (self.blocked) {
        alert.messageText = self.spanish ? @"La actualización está lista" : @"The update is ready";
        alert.informativeText = self.spanish
            ? @"Guarda los cambios pendientes, termina los agentes y cierra las terminales antes de reiniciar. Después pulsa Reiniciar para actualizar."
            : @"Save pending changes, finish agents, and close terminals before restarting. Then click Restart to update.";
        [alert addButtonWithTitle:self.spanish ? @"Continuar trabajando" : @"Keep working"];
        [alert runModal];
        return;
    }
    alert.messageText = self.spanish ? @"¿Reiniciar para actualizar Blackholes?" : @"Restart Blackholes to update?";
    alert.informativeText = self.spanish ? @"La aplicación se cerrará, instalará la actualización y volverá a abrirse. Revisa que no tengas borradores pendientes."
        : @"The app will close, install the update, and reopen. Make sure you have no unsaved drafts.";
    [alert addButtonWithTitle:self.spanish ? @"Más tarde" : @"Later"];
    [alert addButtonWithTitle:self.spanish ? @"Reiniciar y actualizar" : @"Restart and update"];
    if ([alert runModal] == NSAlertSecondButtonReturn && !self.blocked) {
        void (^handler)(void) = self.pendingInstall;
        self.pendingInstall = nil;
        handler();
    }
}
@end

static BHUpdater *bridge;
static NSString *snapshotJSON;

void bh_updater_init(void) {
    NSCAssert(NSThread.isMainThread, @"Updater requires main thread");
    if (bridge) return;
    bridge = [BHUpdater new];
    NSBundle *app = NSBundle.mainBundle;
    NSString *feed = [app objectForInfoDictionaryKey:@"SUFeedURL"];
    NSString *key = [app objectForInfoDictionaryKey:@"SUPublicEDKey"];
    if (![app.bundlePath.pathExtension isEqualToString:@"app"] || ![feed hasPrefix:@"https://"] || key.length == 0) {
        bridge.errorMessage = @"Updates require the packaged release app with its feed and public signing key.";
        return;
    }
    NSBundle *framework = [NSBundle bundleWithPath:[app.privateFrameworksPath stringByAppendingPathComponent:@"Sparkle.framework"]];
    NSError *error = nil;
    if (![framework loadAndReturnError:&error]) {
        bridge.errorMessage = error.localizedDescription ?: @"Sparkle is missing from the application bundle.";
        return;
    }
    Class controllerClass = NSClassFromString(@"SPUStandardUpdaterController");
    if (!controllerClass) { bridge.errorMessage = @"Sparkle updater class is unavailable."; return; }
    bridge.controller = [[controllerClass alloc] initWithStartingUpdater:NO updaterDelegate:bridge userDriverDelegate:bridge];
    // Opt-in installs only. GitHub receives standard HTTPS requests, no account tokens.
    bridge.controller.updater.automaticallyDownloadsUpdates = NO;
    bridge.controller.updater.sendsSystemProfile = NO;
    if (![bridge.controller.updater startUpdater:&error]) {
        bridge.errorMessage = error.localizedDescription;
        bridge.controller = nil;
    }
}

void bh_updater_set_blocked(bool blocked, bool spanish) {
    bridge.blocked = blocked;
    bridge.spanish = spanish;
}

void bh_updater_check(void) {
    NSCAssert(NSThread.isMainThread, @"Updater requires main thread");
    if (bridge.pendingInstall) { [bridge requestRestart]; return; }
    [bridge.controller checkForUpdates:nil];
}

const char *bh_updater_snapshot(void) {
    NSCAssert(NSThread.isMainThread, @"Updater requires main thread");
    NSDictionary *state = @{
        // Objective-C comparisons are ints; @(... != nil) serializes as 0/1,
        // not JSON booleans. Rust deliberately expects true/false here.
        @"enabled": [NSNumber numberWithBool:bridge.controller != nil],
        @"available": bridge.version ?: @"",
        @"busy": [NSNumber numberWithBool:bridge.downloading],
        @"restart": [NSNumber numberWithBool:bridge.pendingInstall != nil],
        @"error": bridge.errorMessage ?: @""
    };
    NSData *data = [NSJSONSerialization dataWithJSONObject:state options:0 error:nil];
    snapshotJSON = [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
    return snapshotJSON.UTF8String;
}
