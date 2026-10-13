#import <Foundation/Foundation.h>
#import "CVirtualDisplay.h"

// Eigene Deklarationen der privaten CoreGraphics-Klassen (Namen und Eigenschaften sind bekannt aus den oeffentlichen
// Klassen-Dumps; die Klassen liegen im CoreGraphics-Framework).
@interface CGVirtualDisplayMode : NSObject
- (instancetype)initWithWidth:(NSUInteger)width height:(NSUInteger)height refreshRate:(double)refreshRate;
@end

@interface CGVirtualDisplaySettings : NSObject
@property(retain, nonatomic) NSArray<CGVirtualDisplayMode *> *modes;
@property(nonatomic) unsigned int hiDPI;
@end

@interface CGVirtualDisplayDescriptor : NSObject
@property(retain, nonatomic) dispatch_queue_t queue;
@property(retain, nonatomic) NSString *name;
@property(nonatomic) unsigned int maxPixelsHigh;
@property(nonatomic) unsigned int maxPixelsWide;
@property(nonatomic) CGSize sizeInMillimeters;
@property(nonatomic) unsigned int productID;
@property(nonatomic) unsigned int vendorID;
@property(nonatomic) unsigned int serialNum;
- (void)setDispatchQueue:(dispatch_queue_t)queue;
@end

@interface CGVirtualDisplay : NSObject
@property(readonly, nonatomic) unsigned int displayID;
- (instancetype)initWithDescriptor:(CGVirtualDisplayDescriptor *)descriptor;
- (BOOL)applySettings:(CGVirtualDisplaySettings *)settings;
@end

struct TPVirtualDisplay {
    void *display;   // CGVirtualDisplay, retained
    void *queue;     // dispatch_queue_t, retained
};

static bool applyMode(CGVirtualDisplay *display, unsigned w, unsigned h, bool hiDPI) {
    CGVirtualDisplaySettings *settings = [[CGVirtualDisplaySettings alloc] init];
    if (settings == nil) return false;
    settings.hiDPI = hiDPI ? 1 : 0;
    CGVirtualDisplayMode *mode = [[CGVirtualDisplayMode alloc] initWithWidth:w height:h refreshRate:60];
    if (mode == nil) return false;
    settings.modes = @[ mode ];
    return [display applySettings:settings];
}

TPVirtualDisplay *TPVirtualDisplayCreate(unsigned widthPoints, unsigned heightPoints, bool hiDPI,
                                         const char *name, unsigned serial) {
    @autoreleasepool {
        Class descClass = NSClassFromString(@"CGVirtualDisplayDescriptor");
        Class dispClass = NSClassFromString(@"CGVirtualDisplay");
        if (descClass == Nil || dispClass == Nil) return NULL;
        CGVirtualDisplayDescriptor *desc = [[descClass alloc] init];
        if (desc == nil) return NULL;
        dispatch_queue_t queue = dispatch_queue_create("beamer.teleport.virtualdisplay", DISPATCH_QUEUE_SERIAL);
        [desc setDispatchQueue:queue];
        desc.name = name ? [NSString stringWithUTF8String:name] : @"Teleport";
        // Obergrenze fuer spaetere Groessenaenderungen (Pixel).
        desc.maxPixelsWide = 7680;
        desc.maxPixelsHigh = 4320;
        double ppi = 110.0;
        desc.sizeInMillimeters = CGSizeMake(widthPoints / ppi * 25.4, heightPoints / ppi * 25.4);
        desc.productID = 0x7E1E;
        desc.vendorID = 0x7E1E;
        desc.serialNum = serial;
        CGVirtualDisplay *display = [[dispClass alloc] initWithDescriptor:desc];
        if (display == nil) return NULL;
        if (!applyMode(display, widthPoints, heightPoints, hiDPI)) return NULL;
        TPVirtualDisplay *r = calloc(1, sizeof(TPVirtualDisplay));
        r->display = (__bridge_retained void *)display;
        r->queue = (__bridge_retained void *)queue;
        return r;
    }
}

CGDirectDisplayID TPVirtualDisplayID(const TPVirtualDisplay *d) {
    if (d == NULL) return 0;
    return ((__bridge CGVirtualDisplay *)d->display).displayID;
}

bool TPVirtualDisplayResize(TPVirtualDisplay *d, unsigned w, unsigned h, bool hiDPI) {
    if (d == NULL) return false;
    return applyMode((__bridge CGVirtualDisplay *)d->display, w, h, hiDPI);
}

void TPVirtualDisplayDestroy(TPVirtualDisplay *d) {
    if (d == NULL) return;
    // Freigeben entfernt den Monitor.
    CFRelease(d->display);
    CFRelease(d->queue);
    free(d);
}
