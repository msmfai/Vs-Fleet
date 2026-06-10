#import <CoreGraphics/CoreGraphics.h>
#import <Foundation/Foundation.h>
#include <string.h>

static NSString *stringValue(NSDictionary *dict, const void *key) {
    id value = dict[(__bridge id)key];
    return [value isKindOfClass:[NSString class]] ? value : @"";
}

static NSNumber *numberValue(NSDictionary *dict, const void *key) {
    id value = dict[(__bridge id)key];
    return [value isKindOfClass:[NSNumber class]] ? value : @0;
}

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        NSString *ownerFilter = nil;
        if (argc > 1 && argv[1] != NULL && strlen(argv[1]) > 0) {
            ownerFilter = [NSString stringWithUTF8String:argv[1]];
        }

        CFArrayRef rawWindows = CGWindowListCopyWindowInfo(kCGWindowListOptionAll, kCGNullWindowID);
        NSArray *windows = CFBridgingRelease(rawWindows);
        NSMutableArray *out = [NSMutableArray array];

        for (NSDictionary *window in windows) {
            NSString *owner = stringValue(window, kCGWindowOwnerName);
            if (ownerFilter != nil && ![owner isEqualToString:ownerFilter]) {
                continue;
            }

            NSDictionary *bounds = window[(__bridge id)kCGWindowBounds];
            if (![bounds isKindOfClass:[NSDictionary class]]) {
                bounds = @{};
            }

            [out addObject:@{
                @"id": numberValue(window, kCGWindowNumber),
                @"owner": owner,
                @"name": stringValue(window, kCGWindowName),
                @"pid": numberValue(window, kCGWindowOwnerPID),
                @"layer": numberValue(window, kCGWindowLayer),
                @"onscreen": numberValue(window, kCGWindowIsOnscreen),
                @"bounds": bounds,
            }];
        }

        NSError *error = nil;
        NSData *json = [NSJSONSerialization dataWithJSONObject:out options:0 error:&error];
        if (json == nil) {
            fprintf(stderr, "failed to serialize windows: %s\n", error.localizedDescription.UTF8String);
            return 2;
        }
        fwrite(json.bytes, 1, json.length, stdout);
        fputc('\n', stdout);
    }
    return 0;
}
