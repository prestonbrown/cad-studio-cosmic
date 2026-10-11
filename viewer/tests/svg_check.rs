use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/golden").join(name)
}

fn render_ink_fraction(svg: &std::path::Path, width: u32) -> (u32, u32, f64) {
    let data = std::fs::read(svg).unwrap();
    let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default())
        .expect("resvg must parse the matplotlib SVG");
    let size = tree.size();
    let height = ((width as f32) * size.height() / size.width()).max(1.0) as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height).unwrap();
    resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pixmap.as_mut());
    let ink = pixmap
        .pixels()
        .iter()
        .filter(|p| p.red() < 240 || p.green() < 240 || p.blue() < 240)
        .count();
    (width, height, ink as f64 / (width as f64 * height as f64))
}

fn png_ink_fraction(png: &std::path::Path, width: u32) -> f64 {
    let img = image::open(png).unwrap().to_rgb8();
    let h = ((width as f64) * img.height() as f64 / img.width() as f64) as u32;
    let img = image::imageops::resize(&img, width, h, image::imageops::FilterType::Nearest);
    let ink = img.pixels().filter(|p| p.0[0] < 240 || p.0[1] < 240 || p.0[2] < 240).count();
    ink as f64 / (width as f64 * h as f64)
}

#[test]
fn matplotlib_svg_renders_with_comparable_ink() {
    let svg = fixture("schematic.svg");
    let png = fixture("schematic.png");
    let (w, h, svg_ink) = render_ink_fraction(&svg, 1600);
    assert!(h > 400, "page height {h} suspiciously small");
    assert!(svg_ink > 0.0005, "almost no ink rendered: {svg_ink}");
    let png_ink = png_ink_fraction(&png, 1600);
    let ratio = svg_ink / png_ink;
    assert!(
        (0.25..4.0).contains(&ratio),
        "resvg ink {svg_ink:.5} vs matplotlib png ink {png_ink:.5} (ratio {ratio:.2}) — resvg likely dropped content"
    );
}

#[test]
fn second_page_svg_parses() {
    let (_w, _h, ink) = render_ink_fraction(&fixture("schematic-2.svg"), 800);
    assert!(ink > 0.0005, "page 2 almost blank under resvg");
}
