use std::{io::Write, path::PathBuf};
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.svg");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let svg = std::fs::read_to_string("assets/icon.svg").unwrap();
    let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).unwrap();
    let mut images = Vec::new();
    for size in [16u32, 24, 32, 48, 64, 128, 256] {
        let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).unwrap();
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(size as f32 / 256.0, size as f32 / 256.0),
            &mut pixmap.as_mut(),
        );
        let png = pixmap.encode_png().unwrap();
        if size == 256 {
            std::fs::write(out.join("icon.png"), &png).unwrap();
        }
        images.push((size, png));
    }
    let mut ico = std::fs::File::create(out.join("thodtalk.ico")).unwrap();
    ico.write_all(&[0, 0, 1, 0, 7, 0]).unwrap();
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, png) in &images {
        ico.write_all(&[*size as u8, *size as u8, 0, 0, 1, 0, 32, 0])
            .unwrap();
        ico.write_all(&(png.len() as u32).to_le_bytes()).unwrap();
        ico.write_all(&offset.to_le_bytes()).unwrap();
        offset += png.len() as u32;
    }
    for (_, png) in images {
        ico.write_all(&png).unwrap();
    }
    drop(ico);
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winres::WindowsResource::new()
            .set_icon(out.join("thodtalk.ico").to_str().unwrap())
            .set("ProductName", "ThodTalk")
            .set("FileDescription", "ThodTalk — общение с друзьями")
            .compile()
            .unwrap();
    }
}
