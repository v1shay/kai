#import <AppKit/AppKit.h>

@interface KaiPetDockTile : NSObject <NSDockTilePlugIn>
@property(nonatomic, weak) NSDockTile *tile;
@property(nonatomic, strong) NSImageView *iconView;
@end

@implementation KaiPetDockTile

- (void)setDockTile:(NSDockTile *)dockTile {
    NSDistributedNotificationCenter *center = [NSDistributedNotificationCenter defaultCenter];
    [center removeObserver:self];
    _tile = dockTile;
    _iconView = nil;
    if (!dockTile) return;
    [center addObserver:self selector:@selector(petChanged:) name:@"com.kai.unified.dock-pet-changed" object:nil];
    [self refreshIcon];
}

- (void)petChanged:(NSNotification *)notification {
    [self refreshIcon];
}

- (void)refreshIcon {
    NSDockTile *tile = self.tile;
    if (!tile) return;
    NSURL *support = [[NSFileManager defaultManager] URLsForDirectory:NSApplicationSupportDirectory
                                                           inDomains:NSUserDomainMask].firstObject;
    NSURL *imageURL = [support URLByAppendingPathComponent:@"Kai Unified/DockPet.png"];
    NSImage *image = [[NSImage alloc] initWithContentsOfURL:imageURL];
    if (!image) {
        tile.contentView = nil;
    } else {
        if (!self.iconView) {
            self.iconView = [[NSImageView alloc] initWithFrame:NSMakeRect(0, 0, tile.size.width, tile.size.height)];
            self.iconView.imageScaling = NSImageScaleProportionallyUpOrDown;
        }
        self.iconView.image = image;
        tile.contentView = self.iconView;
    }
    [tile display];
}

- (void)dealloc {
    [[NSDistributedNotificationCenter defaultCenter] removeObserver:self];
}

@end
