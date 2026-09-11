// AppKit must run on the process main thread, not a Rust test worker. This
// harness tests the production title builder against a disposable status item;
// it never opens Serylane, reads user configuration or starts networking.
#[cfg(target_os = "macos")]
#[path = "../src/traffic_monitor/macos_title.rs"]
#[allow(dead_code)]
mod macos_title;

#[cfg(target_os = "macos")]
fn main() {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
        NSApplication, NSApplicationActivationPolicy, NSAttributedStringNSExtendedStringDrawing,
        NSBitmapImageFileType, NSBitmapImageRep, NSDeviceRGBColorSpace, NSImage, NSStatusBar,
        NSStringDrawingOptions,
    };
    use objc2_foundation::{NSData, NSDictionary, NSSize};

    let mtm = MainThreadMarker::new().expect("native test main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let bar = NSStatusBar::systemStatusBar();
    let item = bar.statusItemWithLength(macos_title::ITEM_WIDTH);
    let button = item.button(mtm).expect("disposable status button");
    let icon = NSImage::initWithData(
        NSImage::alloc(),
        &NSData::with_bytes(include_bytes!("../icons/128x128.png")),
    )
    .unwrap();
    icon.setSize(NSSize::new(18.0, 18.0));
    icon.setTemplate(true);
    button.setImage(Some(&icon));
    let mut text_origin = None;
    for (upload, download) in [
        (0, 0),
        (5939, 6451),
        (1023, 1024),
        (1023 * 1024, 12 * 1024 * 1024),
        (1 << 40, u64::MAX),
    ] {
        macos_title::apply_to_item(&item, upload, download, mtm).unwrap();
        assert_eq!(item.length(), macos_title::ITEM_WIDTH);
        let title = button.attributedTitle();
        assert_eq!(title.string().to_string().lines().count(), 2);
        assert_eq!(title.string().to_string().matches('\t').count(), 6);
        assert!(!button.cell().unwrap().usesSingleLineMode());
        assert_eq!(button.attributedTitle().length(), title.length());
        let bounds = macos_title::build_title(upload, download)
            .boundingRectWithSize_options_context(
                NSSize::new(48.0, 100.0),
                NSStringDrawingOptions::UsesLineFragmentOrigin
                    | NSStringDrawingOptions::UsesFontLeading,
                None,
            );
        assert_eq!(bounds.size.height, macos_title::LINE_HEIGHT * 2.0);
        assert!(
            bounds.size.width <= 42.0,
            "native columns overflow: {bounds:?}"
        );
        assert!(
            bounds.size.width
                <= button
                    .cell()
                    .unwrap()
                    .titleRectForBounds(button.bounds())
                    .size
                    .width
        );
        let origin = button
            .cell()
            .unwrap()
            .titleRectForBounds(button.bounds())
            .origin
            .x;
        assert_eq!(
            origin,
            *text_origin.get_or_insert(origin),
            "unit changes must not shift native columns"
        );
    }
    macos_title::clear_item(&item, mtm).unwrap();
    assert_eq!(button.attributedTitle().length(), 0);
    macos_title::apply_to_item(&item, 5939, 6451, mtm).unwrap();
    assert_eq!(item.length(), macos_title::ITEM_WIDTH);
    println!(
        "native button: titleRect={:?}, font={:?}",
        button.cell().unwrap().titleRectForBounds(button.bounds()),
        button.font().map(|font| font.pointSize())
    );
    let folder =
        std::env::var_os("SERYLANE_NATIVE_TRAY_SNAPSHOT_DIR").map(std::path::PathBuf::from);
    if let Some(folder) = &folder {
        std::fs::create_dir_all(folder).unwrap();
    }
    // Always render in CI too: a string-height assertion alone missed the
    // NSStatusBarButton single-line-baseline clipping regression.
    for scale in [1, 2] {
        for (name, dark, highlighted) in [
            ("light", false, false),
            ("dark", true, false),
            ("selected", false, true),
        ] {
            let appearance = unsafe {
                NSAppearance::appearanceNamed(if dark {
                    NSAppearanceNameDarkAqua
                } else {
                    NSAppearanceNameAqua
                })
            }
            .unwrap();
            button.setAppearance(Some(&appearance));
            button.cell().unwrap().setHighlighted(highlighted);
            let rect = button.bounds();
            let bitmap = unsafe {
                NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
                    NSBitmapImageRep::alloc(), std::ptr::null_mut(),
                    rect.size.width as isize * scale, rect.size.height as isize * scale,
                    8, 4, true, false, NSDeviceRGBColorSpace, 0, 0,
                )
            }.unwrap();
            bitmap.setSize(rect.size);
            button.cacheDisplayInRect_toBitmapImageRep(rect, &bitmap);
            if let Some(folder) = &folder {
                let png = unsafe {
                    bitmap.representationUsingType_properties(
                        NSBitmapImageFileType::PNG,
                        &NSDictionary::new(),
                    )
                }
                .unwrap();
                std::fs::write(
                    folder.join(format!("native-{name}-{scale}x.png")),
                    png.to_vec(),
                )
                .unwrap();
            }
            if !highlighted {
                let text_x =
                    button.cell().unwrap().titleRectForBounds(rect).origin.x as isize * scale;
                let rows: Vec<bool> = (0..bitmap.pixelsHigh())
                    .map(|y| {
                        (text_x..bitmap.pixelsWide())
                            // Offscreen light status buttons carry system
                            // translucency; test ink coverage, not opacity.
                            .any(|x| bitmap.colorAtX_y(x, y).unwrap().alphaComponent() > 0.10)
                    })
                    .collect();
                let runs = rows
                    .iter()
                    .enumerate()
                    .filter(|(y, ink)| **ink && (*y == 0 || !rows[y - 1]))
                    .count();
                if runs != 2 {
                    println!(
                        "ink rows {name} {scale}x: {:?}",
                        rows.iter()
                            .enumerate()
                            .filter(|(_, ink)| **ink)
                            .map(|(y, _)| y)
                            .collect::<Vec<_>>()
                    );
                }
                assert_eq!(
                    runs, 2,
                    "{name} at {scale}x: both rows must be fully visible"
                );
                assert!(
                    !rows[0] && !rows[rows.len() - 1],
                    "{name} at {scale}x: text touches clipping edge"
                );
            }
            println!(
                "native snapshot {name}: bounds={rect:?}, pixels={}x{}",
                bitmap.pixelsWide(),
                bitmap.pixelsHigh()
            );
        }
    }
    bar.removeStatusItem(&item);
    println!("native tray: fixed-width two-row title, unit changes, clear and re-enable passed");
}

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("native macOS tray integration is not applicable on this platform");
}
