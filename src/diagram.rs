//! Mermaid diagrams drawn inline in the terminal.
//!
//! [Pele](https://pele.run) renders the Mermaid source to SVG inside an
//! embedded QuickJS, resvg rasterizes it to PNG, and the Kitty graphics
//! protocol puts the PNG on screen. Everything is set up on the first
//! diagram, so answers without one pay nothing.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use resvg::{tiny_skia, usvg};
use rquickjs::{Context, Runtime};

const PELE_SOURCE: &str = include_str!("../assets/pele/pele.global.js");
/// Pele's own default source limit.
const MAX_SOURCE: usize = 50_000;
/// A runaway diagram stops rendering after this long.
const RENDER_BUDGET: Duration = Duration::from_secs(2);
const JS_MEMORY_LIMIT: usize = 256 * 1024 * 1024;
/// Rendered at twice the diagram's size so it stays sharp on retina screens.
const SCALE: f32 = 2.0;
/// Cap the RGBA image at 16 MiB before allocating its pixel buffer.
const MAX_PIXELS: u64 = 4 * 1024 * 1024;
const MAX_DIMENSION: usize = 8192;
/// Width of a terminal cell in CSS pixels when the terminal doesn't say.
const FALLBACK_CELL_WIDTH: usize = 9;
const KITTY_CHUNK: usize = 4096;

/// Whether this terminal can show images. `WUT_IMAGES=0` or `=1` overrides
/// the guess, which covers terminals known to speak the Kitty protocol.
pub fn images_supported() -> bool {
    match std::env::var("WUT_IMAGES").as_deref() {
        Ok("0") => return false,
        Ok("1") => return true,
        _ => {}
    }
    std::env::var_os("KITTY_WINDOW_ID").is_some()
        || std::env::var("TERM_PROGRAM")
            .is_ok_and(|program| matches!(program.as_str(), "rex" | "ghostty" | "WezTerm"))
}

/// Kitty graphics escapes that draw `source` as an image at most
/// `max_columns` cells wide. The error is a one-line reason, such as Pele's
/// syntax error.
pub fn draw(source: &str, max_columns: usize) -> Result<String, String> {
    let rendered = render_svg(source)?;
    let cell_width = cell_width();
    let max_width = max_columns
        .max(1)
        .saturating_mul(cell_width)
        .saturating_mul(SCALE as usize);
    let png = rasterize(&resolve_colors(&rendered.svg), max_width)?;
    let columns = rendered
        .width
        .div_ceil(cell_width)
        .clamp(1, max_columns.max(1));
    Ok(kitty_escapes(&png, columns))
}

struct Rendered {
    svg: String,
    width: usize,
}

/// Pele loaded into its own QuickJS runtime. `deadline` bounds each render:
/// the interrupt handler stops the script once it passes.
struct Pele {
    _runtime: Runtime,
    context: Context,
    deadline: Rc<Cell<Option<Instant>>>,
}

thread_local! {
    static PELE: RefCell<Option<Pele>> = const { RefCell::new(None) };
    static FONTS: RefCell<Option<usvg::Options<'static>>> = const { RefCell::new(None) };
}

fn render_svg(source: &str) -> Result<Rendered, String> {
    if source.len() > MAX_SOURCE {
        return Err(format!("the diagram is over {MAX_SOURCE} characters"));
    }
    PELE.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Some(load_pele()?);
        }
        let Pele {
            context, deadline, ..
        } = slot.as_ref().expect("Pele is loaded");
        deadline.set(Some(Instant::now() + RENDER_BUDGET));
        let json: Result<String, String> = context.with(|ctx| {
            ctx.globals()
                .set("__wut_source", source)
                .map_err(|error| error.to_string())?;
            ctx.eval::<String, _>(
                r#"(() => {
                    try {
                        const r = pele.render(__wut_source, { responsive: false, padding: 16, images: false });
                        return JSON.stringify({ svg: r.svg, width: r.width });
                    } catch (e) {
                        return JSON.stringify({ error: String((e && e.message) || e) });
                    }
                })()"#,
            )
            .map_err(|error| error.to_string())
        });
        deadline.set(None);
        let value: serde_json::Value =
            serde_json::from_str(&json?).map_err(|error| error.to_string())?;
        if let Some(error) = value["error"].as_str() {
            return Err(first_line(error));
        }
        let svg = value["svg"]
            .as_str()
            .ok_or("Pele returned no SVG")?
            .to_owned();
        let width = value["width"].as_f64().unwrap_or(0.0).ceil().max(1.0) as usize;
        Ok(Rendered { svg, width })
    })
}

fn load_pele() -> Result<Pele, String> {
    let runtime = Runtime::new().map_err(|error| error.to_string())?;
    runtime.set_memory_limit(JS_MEMORY_LIMIT);
    let deadline = Rc::new(Cell::new(None::<Instant>));
    let watched = deadline.clone();
    runtime.set_interrupt_handler(Some(Box::new(move || {
        watched
            .get()
            .is_some_and(|deadline| Instant::now() > deadline)
    })));
    let context = Context::full(&runtime).map_err(|error| error.to_string())?;
    context
        .with(|ctx| ctx.eval::<(), _>(PELE_SOURCE))
        .map_err(|error| format!("could not load Pele: {error}"))?;
    Ok(Pele {
        _runtime: runtime,
        context,
        deadline,
    })
}

fn first_line(message: &str) -> String {
    let mut lines = message
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let first = lines.next().unwrap_or("unknown error").to_owned();
    // Pele's parse errors put the useful part on the last line, such as
    // "Expecting 'NODE_STRING', got 'EOF'".
    match lines.next_back() {
        Some(last) if first.starts_with("Parse error") => format!("{first} {last}"),
        _ => first,
    }
}

/// Replaces CSS custom properties with their values. Pele's root element
/// declares short variables (`--_s:#f3f3f3`) whose values fall back from
/// its public ones (`var(--pele-bg,#fff)`), and shapes use them
/// (`fill="var(--_s)"`). resvg doesn't support custom properties.
fn resolve_colors(svg: &str) -> String {
    let mut svg = svg.to_owned();
    for _ in 0..4 {
        if !svg.contains("var(") {
            break;
        }
        let declared = declarations(&svg);
        svg = substitute(&svg, &declared);
    }
    svg
}

fn declarations(svg: &str) -> HashMap<String, String> {
    svg.split([';', '"'])
        .filter_map(|part| part.trim().split_once(':'))
        .filter(|(name, value)| name.starts_with("--") && !value.contains("var("))
        .map(|(name, value)| (name.to_owned(), value.trim().to_owned()))
        .collect()
}

fn substitute(svg: &str, declared: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    while let Some(start) = rest.find("var(") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 4..];
        let mut depth = 1;
        let mut end = after.len();
        for (index, character) in after.char_indices() {
            match character {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = index;
                        break;
                    }
                }
                _ => {}
            }
        }
        let inner = &after[..end];
        let (name, fallback) = match inner.split_once(',') {
            Some((name, fallback)) => (name.trim(), Some(fallback.trim())),
            None => (inner.trim(), None),
        };
        let consumed = (end + 1).min(after.len());
        match declared.get(name).map(String::as_str).or(fallback) {
            Some(value) => out.push_str(value),
            // Left for the next pass, once its own value is resolved.
            None => out.push_str(&rest[start..start + 4 + consumed]),
        }
        rest = &after[consumed..];
    }
    out.push_str(rest);
    out
}

fn rasterize(svg: &str, max_width: usize) -> Result<Vec<u8>, String> {
    FONTS.with(|cell| {
        let mut slot = cell.borrow_mut();
        let options = slot.get_or_insert_with(|| {
            // SVG references must not bypass workspace file restrictions or
            // introduce independent raster-image allocations before our cap.
            let mut options = usvg::Options {
                image_href_resolver: usvg::ImageHrefResolver {
                    resolve_data: Box::new(|_, _, _| None),
                    resolve_string: Box::new(|_, _| None),
                },
                ..usvg::Options::default()
            };
            options.fontdb_mut().load_system_fonts();
            for family in ["Helvetica", "Arial", "DejaVu Sans", "Liberation Sans"] {
                if options
                    .fontdb
                    .faces()
                    .any(|face| face.families.iter().any(|(name, _)| name == family))
                {
                    options.fontdb_mut().set_sans_serif_family(family);
                    options.font_family = family.to_owned();
                    break;
                }
            }
            options
        });
        let tree = usvg::Tree::from_str(svg, options)
            .map_err(|error| format!("could not read Pele's SVG: {error}"))?;
        let (size, scale) = raster_geometry(tree.size().width(), tree.size().height(), max_width)?;
        let mut pixmap = tiny_skia::Pixmap::new(size.width(), size.height())
            .ok_or("the diagram is too large to draw")?;
        // The SVG has no background of its own, and dark lines would vanish
        // on a dark terminal. Pele's default background is white.
        pixmap.fill(tiny_skia::Color::WHITE);
        resvg::render(
            &tree,
            tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        pixmap
            .encode_png()
            .map_err(|error| format!("could not encode the diagram: {error}"))
    })
}

/// Match the terminal's display width before allocating, then reject tall or
/// oversized canvases. The caller falls back to the original Mermaid text.
fn raster_geometry(
    width: f32,
    height: f32,
    max_width: usize,
) -> Result<(tiny_skia::IntSize, f32), String> {
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return Err("the diagram is empty".to_owned());
    }
    let max_width = max_width.clamp(1, MAX_DIMENSION);
    let scale = f64::from(SCALE).min(max_width as f64 / f64::from(width));
    let pixels_wide = (f64::from(width) * scale).ceil().min(max_width as f64) as u32;
    let pixels_high = (f64::from(height) * scale).ceil();
    if pixels_high > MAX_DIMENSION as f64
        || u64::from(pixels_wide) * pixels_high as u64 > MAX_PIXELS
    {
        return Err("the diagram is too large to draw".to_owned());
    }
    let size = tiny_skia::IntSize::from_wh(pixels_wide, pixels_high as u32)
        .ok_or("the diagram is empty")?;
    Ok((size, scale as f32))
}

/// The width of a terminal cell in CSS pixels, from the terminal's reported
/// window size. Retina terminals report device pixels, twice the CSS size.
fn cell_width() -> usize {
    let Ok(size) = crossterm::terminal::window_size() else {
        return FALLBACK_CELL_WIDTH;
    };
    if size.columns == 0 || size.width == 0 {
        return FALLBACK_CELL_WIDTH;
    }
    let cell = usize::from(size.width) / usize::from(size.columns);
    match cell {
        0..=3 => FALLBACK_CELL_WIDTH,
        4..=13 => cell,
        _ => cell / 2,
    }
}

/// Kitty graphics escapes that transmit and show `png`, `columns` cells
/// wide. `q=2` keeps the terminal from answering on stdin.
fn kitty_escapes(png: &[u8], columns: usize) -> String {
    let data = base64(png);
    let chunks: Vec<&str> = data
        .as_bytes()
        .chunks(KITTY_CHUNK)
        .map(|chunk| std::str::from_utf8(chunk).expect("base64 is ASCII"))
        .collect();
    let mut escapes = String::with_capacity(data.len() + chunks.len() * 16);
    for (index, chunk) in chunks.iter().enumerate() {
        let more = u8::from(index + 1 < chunks.len());
        if index == 0 {
            escapes.push_str(&format!("\x1b_Ga=T,f=100,q=2,c={columns},m={more};"));
        } else {
            escapes.push_str(&format!("\x1b_Gm={more};"));
        }
        escapes.push_str(chunk);
        escapes.push_str("\x1b\\");
    }
    escapes
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        let digit = |shift: u32| char::from(TABLE[((n >> shift) & 63) as usize]);
        encoded.push(digit(18));
        encoded.push(digit(12));
        encoded.push(if chunk.len() > 1 { digit(6) } else { '=' });
        encoded.push(if chunk.len() > 2 { digit(0) } else { '=' });
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::{
        KITTY_CHUNK, MAX_DIMENSION, MAX_PIXELS, base64, draw, first_line, kitty_escapes,
        raster_geometry, rasterize, render_svg, resolve_colors,
    };

    const FLOWCHART: &str = "flowchart LR\n  A[Question] --> B{Tools?}\n  B -- yes --> C[grep]\n  B -- no --> D[Answer]\n  C --> D";

    #[test]
    fn pele_renders_a_flowchart() {
        let rendered = render_svg(FLOWCHART).unwrap();
        assert!(rendered.svg.starts_with("<svg"), "{}", &rendered.svg[..60]);
        assert!(rendered.svg.contains("Question"));
        assert!(rendered.width > 100, "{}", rendered.width);
    }

    #[test]
    fn syntax_errors_come_back_as_one_line() {
        let error = render_svg("flowchart LR\n  A --> ").err().unwrap();
        assert!(error.starts_with("Parse error on line 2"), "{error}");
        assert!(error.contains("Expecting"), "{error}");
        assert!(!error.contains('\n'), "{error}");
        assert!(render_svg("not a diagram").is_err());
    }

    #[test]
    fn mermaid_image_nodes_keep_their_labels_without_image_references() {
        let rendered =
            render_svg("flowchart LR\n A@{ img: \"private.svg\", label: \"Picture\", h: 40 }")
                .unwrap();
        assert!(rendered.svg.contains("Picture"));
        assert!(!rendered.svg.contains("<image"));
    }

    #[test]
    fn resolves_every_variable() {
        let svg = resolve_colors(
            r#"<svg style="--_bg:var(--pele-bg,#fff);--_s:var(--pele-surface,#f3f3f3);--_r:var(--pele-radius,4px)"><rect fill="var(--_s)" style="rx:var(--_r)"/><text fill="var(--_missing, rgb(1,2,3))"/></svg>"#,
        );
        assert!(!svg.contains("var("), "{svg}");
        assert!(svg.contains(r##"fill="#f3f3f3""##), "{svg}");
        assert!(svg.contains("rx:4px"), "{svg}");
        assert!(svg.contains("rgb(1,2,3)"), "{svg}");
        let rendered = resolve_colors(&render_svg(FLOWCHART).unwrap().svg);
        assert!(!rendered.contains("var("));
    }

    #[test]
    fn rasterizes_at_twice_the_size() {
        let rendered = render_svg(FLOWCHART).unwrap();
        let png = rasterize(&resolve_colors(&rendered.svg), MAX_DIMENSION).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let width = u32::from_be_bytes(png[16..20].try_into().unwrap()) as usize;
        assert!(
            width.abs_diff(rendered.width * 2) <= 2,
            "{width} vs {}",
            rendered.width
        );
    }

    #[test]
    fn wide_diagrams_are_rasterized_at_the_display_width() {
        let png = rasterize(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="5000" height="400"><rect width="5000" height="400"/></svg>"#,
            640,
        )
        .unwrap();
        let width = u32::from_be_bytes(png[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(png[20..24].try_into().unwrap());
        assert_eq!(width, 640);
        assert_eq!(height, 52);
        assert!(u64::from(width) * u64::from(height) <= MAX_PIXELS);
    }

    #[test]
    fn image_limits_are_checked_before_pixel_allocation() {
        assert!(raster_geometry(2000.0, 2000.0, MAX_DIMENSION).is_err());
        assert!(raster_geometry(1.0, 100_000_000.0, 640).is_err());
        assert!(raster_geometry(f32::MAX, f32::MAX, usize::MAX).is_err());
        for invalid in [0.0, -1.0, f32::INFINITY, f32::NAN] {
            assert!(raster_geometry(invalid, 1.0, 640).is_err());
            assert!(raster_geometry(1.0, invalid, 640).is_err());
        }
        let (size, _) = raster_geometry(1024.0, 1024.0, usize::MAX).unwrap();
        assert_eq!(
            u64::from(size.width()) * u64::from(size.height()),
            MAX_PIXELS
        );
        let (size, _) = raster_geometry(10.0, 10.0, 0).unwrap();
        assert_eq!(size.width(), 1);
    }

    #[test]
    fn svg_images_cannot_load_local_files_or_embedded_images() {
        let image = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="red"/></svg>"#;
        let path = std::env::temp_dir().join(format!(
            "wut-diagram-private-{}-{}.svg",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, image).unwrap();
        let hrefs = [
            path.to_string_lossy().into_owned(),
            format!("data:image/svg+xml;base64,{}", base64(image.as_bytes())),
        ];
        let results: Vec<_> = hrefs
            .iter()
            .map(|href| {
                let href = href.replace('&', "&amp;").replace('"', "&quot;");
                rasterize(
                    &format!(
                        r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><image href="{href}" width="10" height="10"/></svg>"#
                    ),
                    40,
                )
            })
            .collect();
        std::fs::remove_file(path).unwrap();
        for result in results {
            let pixels = super::tiny_skia::Pixmap::decode_png(&result.unwrap()).unwrap();
            assert!(pixels.pixels().iter().all(|pixel| {
                pixel.red() == 255 && pixel.green() == 255 && pixel.blue() == 255
            }));
        }
    }

    #[test]
    fn kitty_escapes_are_chunked() {
        let png = vec![7u8; 10_000];
        let escapes = kitty_escapes(&png, 40);
        assert!(escapes.starts_with("\x1b_Ga=T,f=100,q=2,c=40,m=1;"));
        assert!(escapes.ends_with("\x1b\\"));
        let chunks: Vec<&str> = escapes
            .split("\x1b\\")
            .filter(|chunk| !chunk.is_empty())
            .collect();
        assert_eq!(chunks.len(), base64(&png).len().div_ceil(KITTY_CHUNK));
        assert!(chunks.last().unwrap().starts_with("\x1b_Gm=0;"));
        for chunk in &chunks {
            let payload = chunk.split_once(';').unwrap().1;
            assert!(payload.len() <= KITTY_CHUNK);
        }
        let small = kitty_escapes(b"x", 3);
        assert_eq!(small, "\x1b_Ga=T,f=100,q=2,c=3,m=0;eA==\x1b\\");
    }

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn draw_fits_the_terminal() {
        let escapes = draw(FLOWCHART, 10).unwrap();
        assert!(escapes.starts_with("\x1b_Ga=T,f=100,q=2,c=10,"));
    }

    #[test]
    fn keeps_short_errors_whole() {
        assert_eq!(
            first_line("No diagram type detected"),
            "No diagram type detected"
        );
        assert_eq!(first_line(""), "unknown error");
    }
}
