//! HTML bounding-box overlay preview.
//!
//! Writes a single self-contained `.preview.html` next to the JSON showing every
//! extracted box drawn on the page it came from, so extraction quality is
//! visible instead of inferred from numbers.
//!
//! Two things make a box readable:
//!
//! * the box itself, with the recognised text drawn inside it at the size the
//!   box implies — a text run whose box is too wide or too narrow is obvious
//!   immediately, and
//! * a linked sidebar, so hovering a row highlights its box and vice versa.
//!
//! A page backdrop is used when one is available. Raster pages (scans) already
//! have their image extracted, and for text pages `pdftoppm` is used **when it
//! happens to be installed**. Rendering is best-effort and preview-only: the
//! extractor itself never depends on an external renderer, and the HTML is
//! complete without one (it then shows the extracted text reconstructed at its
//! true coordinates, which is what the parser actually believes the page says).

use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Which stage produced a box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayKind {
    /// Text from the PDF's own text layer.
    Text,
    /// Text produced by OCR of a page image.
    Ocr,
    /// A figure drawn with vector paths.
    Figure,
    /// A raster image XObject.
    Image,
    /// A block of reading order: the region layout analysis assigned one
    /// element to. It carries no text of its own, because its member boxes
    /// already draw theirs.
    Block,
}

impl OverlayKind {
    fn slug(self) -> &'static str {
        match self {
            OverlayKind::Text => "text",
            OverlayKind::Ocr => "ocr",
            OverlayKind::Figure => "figure",
            OverlayKind::Image => "image",
            OverlayKind::Block => "block",
        }
    }

    fn title(self) -> &'static str {
        match self {
            OverlayKind::Text => "text layer",
            OverlayKind::Ocr => "OCR",
            OverlayKind::Figure => "vector figure",
            OverlayKind::Image => "image",
            OverlayKind::Block => "block",
        }
    }

    fn color(self) -> &'static str {
        match self {
            OverlayKind::Text => "#1d4ed8",
            OverlayKind::Ocr => "#b45309",
            OverlayKind::Figure => "#15803d",
            OverlayKind::Image => "#7e22ce",
            OverlayKind::Block => "#0f766e",
        }
    }
}

/// One drawn box.
#[derive(Debug, Clone)]
pub struct OverlayItem {
    pub page_no: i32,
    /// `[l, t, r, b]` in points, top-left origin.
    pub bbox: [f64; 4],
    pub kind: OverlayKind,
    pub label: String,
    pub text: String,
    /// Relative path to the extracted image, for `Image` items.
    pub image_uri: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PreviewPage {
    pub page_no: i32,
    pub width: f64,
    pub height: f64,
    /// Rendered page backdrop, relative to the HTML file.
    pub backdrop: Option<String>,
}

/// Everything the overlay needs, collected while the document is built.
#[derive(Debug, Default)]
pub struct Preview {
    pub pages: Vec<PreviewPage>,
    pub items: Vec<OverlayItem>,
}

impl Preview {
    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    pub fn count(&self, kind: OverlayKind) -> usize {
        self.items.iter().filter(|i| i.kind == kind).count()
    }

    /// Write the overlay HTML. `name` is the document name for the heading.
    pub fn write(&self, path: &Path, name: &str) -> io::Result<()> {
        std::fs::write(path, self.to_html(name))
    }

    /// Build the whole document as one string.
    pub fn to_html(&self, name: &str) -> String {
        let mut html = String::with_capacity(64 * 1024 + self.items.len() * 220);
        let rendered = self.pages.iter().filter(|p| p.backdrop.is_some()).count();

        html.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
        html.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
        let _ = writeln!(html, "<title>{} — box overlay</title>", esc(name));
        html.push_str(STYLE);
        html.push_str("</head>\n<body class=\"show-text\">\n");

        // --- header -------------------------------------------------------
        html.push_str("<header>\n");
        let _ = writeln!(html, "<h1>{}</h1>", esc(name));
        html.push_str("<div class=\"meta\">");
        let _ = write!(
            html,
            "{} page{} · {} text · {} OCR · {} figure · {} image · {} block",
            self.pages.len(),
            if self.pages.len() == 1 { "" } else { "s" },
            self.count(OverlayKind::Text),
            self.count(OverlayKind::Ocr),
            self.count(OverlayKind::Figure),
            self.count(OverlayKind::Image),
            self.count(OverlayKind::Block),
        );
        if rendered > 0 {
            let _ = write!(html, " · {rendered} rendered");
        } else {
            html.push_str(" · text reconstructed");
        }
        html.push_str("</div>\n");

        html.push_str("<div class=\"controls\">\n");
        for kind in [
            OverlayKind::Text,
            OverlayKind::Ocr,
            OverlayKind::Figure,
            OverlayKind::Image,
            OverlayKind::Block,
        ] {
            let _ = writeln!(
                html,
                "<label class=\"tog\"><input type=\"checkbox\" checked data-kind=\"{}\">\
                 <i style=\"background:{}\"></i>{}</label>",
                kind.slug(),
                kind.color(),
                kind.title(),
            );
        }
        html.push_str(
            "<label class=\"tog\"><input type=\"checkbox\" id=\"sidebar\" checked>items</label>\n\
             <label class=\"tog\"><input type=\"checkbox\" checked data-toggle=\"text\">show text</label>\n\
             <label class=\"tog\"><input type=\"checkbox\" data-toggle=\"clip\">clip text</label>\n\
             <label class=\"tog\">zoom <input id=\"zoom\" type=\"range\" min=\"25\" max=\"400\" value=\"115\">\
             <span id=\"zoomv\">115%</span></label>\n",
        );
        html.push_str("</div>\n</header>\n");

        // --- sidebar + pages ----------------------------------------------
        html.push_str("<main>\n<aside id=\"list\">\n");
        if self.items.is_empty() {
            html.push_str("<div class=\"empty\">No boxes were extracted.</div>\n");
        }
        let mut current_page = None;
        for (i, item) in self.items.iter().enumerate() {
            if current_page != Some(item.page_no) {
                if current_page.is_some() {
                    html.push_str("</div>\n");
                }
                current_page = Some(item.page_no);
                let _ = write!(
                    html,
                    "<div class=\"pgsec\" data-pg=\"{}\">Page {}</div>\n<div>",
                    item.page_no, item.page_no
                );
            }
            // Boxes without text (figures, images) still need a readable row.
            let shown = if item.text.trim().is_empty() {
                item.label.clone()
            } else {
                item.text.clone()
            };
            let chip = if item.label != "paragraph" {
                format!("<span class=\"lb\">{}</span>", esc(&item.label))
            } else {
                String::new()
            };
            let _ = writeln!(
                html,
                "<div class=\"row\" data-i=\"{i}\" data-kind=\"{}\" data-label=\"{}\">\
                 <i class=\"sw\" style=\"background:{}\"></i>\
                 <div class=\"txt\">{}{}</div></div>",
                item.kind.slug(),
                esc(&item.label),
                item.kind.color(),
                esc(&shown),
                chip,
            );
        }
        if current_page.is_some() {
            html.push_str("</div>\n");
        }
        html.push_str("</aside>\n<section id=\"pages\">\n");

        for page in &self.pages {
            let _ = writeln!(
                html,
                "<div class=\"pagewrap\"><div class=\"pn\">Page {}</div>\
                 <div class=\"page\" data-pg=\"{}\" data-pw=\"{:.2}\" 
                 style=\"width:calc(var(--u)*{:.2});height:calc(var(--u)*{:.2})\">",
                page.page_no, page.page_no, page.width, page.width, page.height
            );
            if let Some(uri) = &page.backdrop {
                let _ = writeln!(
                    html,
                    "<img class=\"backdrop\" src=\"{}\" alt=\"\">",
                    esc(uri)
                );
            }
            for (index, item) in self
                .items
                .iter()
                .enumerate()
                .filter(|(_, i)| i.page_no == page.page_no)
            {
                let [l, t, r, b] = item.bbox;
                let (w, h) = ((r - l).max(0.0), (b - t).max(0.0));
                // Undrawn content (an image or figure already shown as the page
                // backdrop) only needs its outline.
                let class = format!("box kind-{}", item.kind.slug());
                if item.kind == OverlayKind::Image {
                    if let (Some(uri), false) = (&item.image_uri, page.backdrop.is_some()) {
                        let _ = writeln!(
                            html,
                            "<img class=\"{class}\" src=\"{}\" style=\"{}\" alt=\"\">",
                            esc(uri),
                            pos(l, t, w, h)
                        );
                    }
                }
                let _ = write!(
                    html,
                    "<div class=\"{class}\" data-i=\"{index}\" style=\"{}{}\" title=\"{}\">",
                    pos(l, t, w, h),
                    if item.kind == OverlayKind::Text || item.kind == OverlayKind::Ocr {
                        // A typeface's em is roughly 0.72 of a single-spaced line
                        // box, so this matches the original glyph size instead of
                        // overflowing it.
                        let size = (h * 0.72).clamp(3.0, 200.0);
                        format!("font-size:calc(var(--u)*{size:.2})")
                    } else {
                        String::new()
                    },
                    esc(&item.text),
                );
                if item.kind == OverlayKind::Block {
                    // The block's text is already drawn by the runs inside it;
                    // repeating it here would double every glyph.
                    let _ = write!(html, "<span class=\"lb\">{}</span>", esc(&item.label));
                } else if !item.text.is_empty() {
                    let _ = write!(html, "<span>{}</span>", esc(&item.text));
                } else if item.kind != OverlayKind::Image {
                    // Figures and the like carry no text: name them on the page.
                    let _ = write!(html, "<span class=\"lb\">{}</span>", esc(&item.label));
                }
                html.push_str("</div>\n");
            }
            html.push_str("</div></div>\n");
        }
        html.push_str("</section>\n</main>\n");
        html.push_str(SCRIPT);
        html.push_str("</body>\n</html>\n");
        html
    }
}

/// The geometry declarations, terminated so callers can append more.
///
/// The trailing `;` is load-bearing: without it an appended `font-size` merges
/// into the `height` declaration and the browser discards both.
fn pos(l: f64, t: f64, w: f64, h: f64) -> String {
    format!(
        "left:calc(var(--u)*{l:.2});top:calc(var(--u)*{t:.2});\
         width:calc(var(--u)*{w:.2});height:calc(var(--u)*{h:.2});"
    )
}

/// Escape text for HTML text and attribute contexts.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            // Keep tabs visible and drop other control characters.
            '\n' | '\t' => out.push(' '),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}

/// Render each page to PNG with `pdftoppm`, if it is installed.
///
/// Returns one entry per page in `pages`, holding the path relative to the HTML
/// file (assumed to live in the same directory as `out_dir`'s parent). A missing
/// renderer is not an error: the caller falls back to a text reconstruction.
pub fn render_pages(
    pdf: &Path,
    out_dir: &Path,
    pages: &[PreviewPage],
    dpi: u32,
) -> Vec<Option<String>> {
    let mut result: Vec<Option<String>> = vec![None; pages.len()];
    if pages.is_empty() || which("pdftoppm").is_none() {
        return result;
    }
    if std::fs::create_dir_all(out_dir).is_err() {
        return result;
    }

    // One invocation for the whole document; `pdftoppm` zero-pads based on the
    // page count, so the files are matched by their trailing page number rather
    // than by a guessed pattern.
    let prefix = out_dir.join("page");
    let status = Command::new("pdftoppm")
        .arg("-png")
        .arg("-r")
        .arg(dpi.to_string())
        .arg(pdf)
        .arg(&prefix)
        .status();

    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            tracing::debug!("pdftoppm exited with {s}; preview will omit page images");
            return result;
        }
        Err(err) => {
            tracing::debug!("pdftoppm could not run ({err}); preview will omit page images");
            return result;
        }
    }

    let dir_name = out_dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("render");
    let Ok(entries) = std::fs::read_dir(out_dir) else {
        return result;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("png") {
            continue;
        }
        let Some(number) = trailing_number(&path) else {
            continue;
        };
        let Some(slot) = pages.iter().position(|p| p.page_no == number) else {
            continue;
        };
        if let Some(file) = path.file_name().and_then(|f| f.to_str()) {
            result[slot] = Some(format!("{dir_name}/{file}"));
        }
    }
    result
}

/// The trailing integer of a file stem, e.g. `page-012` -> 12.
fn trailing_number(path: &Path) -> Option<i32> {
    let stem = path.file_stem()?.to_str()?;
    let digits: String = stem
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    digits.parse().ok()
}

/// Minimal `which`: is `bin` an executable file on `PATH`?
fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(bin))
        .find(|candidate| candidate.is_file())
}

const STYLE: &str = r#"<style>
:root{
  /* Initial px-per-point. Real value is fitted to the panel on load; this is
     only the no-script fallback, so it errs small rather than overflowing. */
  --u:0.9px;
  --text:#1d4ed8; --ocr:#b45309; --figure:#15803d; --image:#7e22ce; --block:#0f766e;
  --line:#d1d5db;
}
*{box-sizing:border-box}
html,body{margin:0}
body{font:13px/1.45 system-ui,-apple-system,Segoe UI,Roboto,sans-serif;background:#eef0f3;color:#111827}
body:not(.show-text) .box span{display:none}
body.hide-text .kind-text,body.hide-ocr .kind-ocr,
body.hide-figure .kind-figure,body.hide-image .kind-image,
body.hide-block .kind-block{display:none}
body.hide-text .row[data-kind=text],body.hide-ocr .row[data-kind=ocr],
body.hide-figure .row[data-kind=figure],body.hide-image .row[data-kind=image],
body.hide-block .row[data-kind=block]{opacity:.28}
header{position:sticky;top:0;z-index:20;display:flex;flex-wrap:wrap;align-items:center;gap:8px 16px;
  padding:9px 14px;background:#fff;border-bottom:1px solid var(--line);box-shadow:0 1px 3px rgba(0,0,0,.06)}
h1{margin:0;font-size:14px;font-weight:650}
.meta{color:#4b5563;font-variant-numeric:tabular-nums}
.controls{display:flex;flex-wrap:wrap;gap:6px 14px;margin-left:auto;align-items:center}
.tog{display:inline-flex;align-items:center;gap:5px;cursor:pointer;user-select:none;white-space:nowrap}
.tog i{width:10px;height:10px;border-radius:2px;display:inline-block}
#zoom{width:130px}
main{display:flex;align-items:flex-start;gap:14px;padding:14px}
aside{flex:0 0 340px;width:340px;position:sticky;top:58px;max-height:calc(100vh - 72px);overflow:auto;
  background:#fff;border:1px solid var(--line);border-radius:8px}
aside .empty{padding:12px;color:#6b7280}
.pgsec{position:sticky;top:0;background:#f9fafb;border-bottom:1px solid var(--line);
  padding:5px 9px;font-size:11px;font-weight:650;letter-spacing:.04em;text-transform:uppercase;color:#6b7280}
.row{display:flex;gap:7px;padding:5px 9px;border-bottom:1px solid #f3f4f6;align-items:flex-start}
.sw{flex:0 0 9px;width:9px;height:9px;border-radius:2px;margin-top:4px}
.lb{display:inline-block;font-size:9px;line-height:1.5;padding:0 4px;margin-right:5px;border-radius:3px;
  background:currentColor;color:#fff;text-transform:uppercase;letter-spacing:.04em;vertical-align:top}
.box .lb{position:absolute;top:0;left:0}
.row .txt{flex:1;min-width:0;overflow-wrap:anywhere;font-size:12px}
.row.hot{background:#fef9c3}
.row.hot .txt{font-weight:600}
/* `align-self:stretch` matters: with the default `flex-start`, a stacked panel
   would size this column to its content, so the page could never fit. */
#pages{flex:1;min-width:0;align-self:stretch;display:flex;flex-direction:column;gap:22px;align-items:flex-start}
.pagewrap{position:relative}
.pn{font-size:11px;color:#6b7280;margin-bottom:3px}
.page{position:relative;background:#fff;box-shadow:0 1px 5px rgba(0,0,0,.2);overflow:hidden}
.page img.backdrop{position:absolute;inset:0;width:100%;height:100%;display:block}
.box{position:absolute;border:1px solid currentColor;background:rgba(29,78,216,.05);overflow:visible}
.box span{display:block;white-space:pre;line-height:1.12;color:currentColor;
  text-shadow:0 0 2px #fff,0 0 2px #fff,0 0 3px #fff}
/* With the extracted text drawn on top, the page render is faded so the two can
   be compared instead of fighting each other. Toggling text off restores it. */
body.show-text .page img.backdrop,body.show-text img.box.kind-image{opacity:.28}
/* Optional: clip each run to its box, which makes a too-narrow estimated box
   read as truncation instead of overflow. */
body.clip .box{overflow:hidden}
body.clip .box span{overflow:hidden}
.box.hot{outline:2px solid #f59e0b;outline-offset:0;z-index:5;background:rgba(250,204,21,.3)}
.kind-text{color:var(--text)}
.kind-ocr{color:var(--ocr);background:rgba(180,83,9,.07)}
.kind-figure{color:var(--figure);background:rgba(21,128,61,.06);border-style:dashed}
/* A block is the region layout assigned one element to, so its outline is drawn
   dotted behind the runs it contains. */
.kind-block{color:var(--block);background:rgba(15,118,110,.04);border-style:dotted}
.kind-image{color:var(--image);object-fit:fill}
.box.kind-image{background:rgba(126,34,206,.05)}
body.no-side aside{display:none}
/* A narrow panel needs the full width for the page, so the item list moves
   above it instead of stealing a fixed 340px column. */
@media (max-width:1000px){
  main{flex-direction:column}
  aside{position:static;width:100%;flex:none;max-height:34vh}
}
</style>
"#;

const SCRIPT: &str = r#"<script>
(function(){
  var root=document.documentElement;
  var zoom=document.getElementById('zoom');
  var zoomv=document.getElementById('zoomv');
  // The scale is px-per-point; the slider shows it as a percentage.
  function applyZoom(){
    root.style.setProperty('--u',(zoom.value/100)+'px');
    zoomv.textContent=zoom.value+'%';
  }
  var pageWidths=Array.prototype.map.call(document.querySelectorAll('.page'),function(p){
    return parseFloat(p.dataset.pw)||612;
  });
  var widest=pageWidths.length?Math.max.apply(null,pageWidths):612;
  // Fit to the panel on load: a fixed scale would push the page out of view in
  // a narrow window, which is exactly where the overlay is most useful.
  var autofit=true;
  function fit(){
    var sec=document.getElementById('pages');
    var avail=sec.clientWidth-6;
    var u=avail/widest;
    var pct=Math.max(25,Math.min(400,Math.round(u*100)));
    zoom.value=pct;
    applyZoom();
  }
  function refit(){ if(autofit){ fit(); } }

  zoom.addEventListener('input',function(){autofit=false;applyZoom();});
  var sb=document.getElementById('sidebar');
  if(sb){ sb.addEventListener('change',function(){document.body.classList.toggle('no-side',!sb.checked);refit();}); }
  window.addEventListener('resize',refit);
  fit();

  document.querySelectorAll('input[data-kind]').forEach(function(cb){
    cb.addEventListener('change',function(){
      document.body.classList.toggle('hide-'+cb.dataset.kind,!cb.checked);
    });
  });
  document.querySelectorAll('input[data-toggle]').forEach(function(cb){
    cb.addEventListener('change',function(){
      document.body.classList.toggle(cb.dataset.toggle==='clip'?'clip':'show-text',cb.checked);
    });
  });
  // Fonts and images can change the layout after first paint.
  window.addEventListener('load',refit);
  if(document.fonts&&document.fonts.ready){document.fonts.ready.then(refit);}

  function hot(index,on){
    document.querySelectorAll('[data-i="'+index+'"]').forEach(function(el){
      el.classList.toggle('hot',on);
    });
  }
  document.querySelectorAll('.box,.row').forEach(function(el){
    el.addEventListener('mouseenter',function(){hot(el.dataset.i,true);});
    el.addEventListener('mouseleave',function(){hot(el.dataset.i,false);});
  });
  document.querySelectorAll('.row').forEach(function(el){
    el.addEventListener('click',function(){
      var box=document.querySelector('.box[data-i="'+el.dataset.i+'"]');
      if(box){box.scrollIntoView({block:'center',inline:'center',behavior:'smooth'});}
    });
  });
})();
</script>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn item(page_no: i32, text: &str, kind: OverlayKind) -> OverlayItem {
        OverlayItem {
            page_no,
            bbox: [10.0, 20.0, 110.0, 32.0],
            kind,
            label: "paragraph".to_string(),
            text: text.to_string(),
            image_uri: None,
        }
    }

    fn preview() -> Preview {
        Preview {
            pages: vec![PreviewPage {
                page_no: 1,
                width: 612.0,
                height: 792.0,
                backdrop: Some("doc_images/page-1.png".to_string()),
            }],
            items: vec![
                item(1, "Hello World", OverlayKind::Text),
                item(1, "ACME Supplies", OverlayKind::Ocr),
            ],
        }
    }

    /// Every declaration in every inline style must be independently valid.
    /// A missing separator silently merges two declarations, and the browser
    /// drops both — a box then loses its height and its font size, which is
    /// invisible in the markup and obvious only once rendered.
    #[test]
    fn inline_styles_have_well_formed_declarations() {
        let html = preview().to_html("demo.pdf");
        let mut checked = 0;
        for style in html
            .split("style=\"")
            .skip(1)
            .filter_map(|s| s.split('"').next())
        {
            checked += 1;
            for decl in style.split(';') {
                let decl = decl.trim();
                if decl.is_empty() {
                    continue;
                }
                assert_eq!(
                    decl.matches(':').count(),
                    1,
                    "merged or invalid declaration {decl:?} in style {style:?}"
                );
                let (prop, value) = decl.split_once(':').expect("a colon");
                assert!(
                    !prop.is_empty() && !value.trim().is_empty(),
                    "empty {decl:?}"
                );
            }
        }
        assert!(
            checked > 2,
            "expected inline styles to check, saw {checked}"
        );
    }

    #[test]
    fn a_text_box_keeps_both_its_height_and_its_font_size() {
        let html = preview().to_html("demo.pdf");
        // 12pt tall box -> 8.64pt font, and the height must still be declared.
        assert!(
            html.contains("height:calc(var(--u)*12.00);font-size:calc(var(--u)*8.64)"),
            "height and font-size were not both kept: {html}"
        );
    }

    #[test]
    fn writes_boxes_at_their_true_coordinates() {
        let html = preview().to_html("demo.pdf");
        // 10pt left, 20pt top, 100pt wide, 12pt tall, scaled through --u.
        assert!(html.contains("left:calc(var(--u)*10.00)"), "{html}");
        assert!(html.contains("top:calc(var(--u)*20.00)"), "{html}");
        assert!(html.contains("width:calc(var(--u)*100.00)"), "{html}");
        assert!(html.contains("height:calc(var(--u)*12.00)"), "{html}");
        // The page itself is sized in points.
        assert!(html.contains("width:calc(var(--u)*612.00)"), "{html}");
        assert!(html.contains("height:calc(var(--u)*792.00)"), "{html}");
    }

    #[test]
    fn page_width_is_exposed_so_the_view_can_fit_the_panel() {
        let html = preview().to_html("demo.pdf");
        assert!(html.contains("data-pw=\"612.00\""), "{html}");
        // A fixed default scale would push the page out of a narrow panel.
        assert!(html.contains("function fit()"), "no fit-to-width wiring");
        assert!(
            html.contains("addEventListener('resize'"),
            "no refit on resize"
        );
        assert!(html.contains("max-width:1000px"), "no narrow-panel layout");
    }

    #[test]
    fn text_is_visible_by_default() {
        let html = preview().to_html("demo.pdf");
        // The whole point of the overlay is seeing the extracted text in place.
        assert!(html.contains("<body class=\"show-text\">"), "{html}");
        assert!(html.contains("checked data-toggle=\"text\""), "{html}");
    }

    #[test]
    fn distinguishes_text_layer_from_ocr_boxes() {
        let html = preview().to_html("demo.pdf");
        assert!(html.contains("class=\"box kind-text\""), "{html}");
        assert!(html.contains("class=\"box kind-ocr\""), "{html}");
        assert!(html.contains("data-kind=\"ocr\""), "{html}");
    }

    #[test]
    fn escapes_text_so_the_page_cannot_inject_markup() {
        let html = Preview {
            pages: vec![PreviewPage {
                page_no: 1,
                width: 100.0,
                height: 100.0,
                backdrop: None,
            }],
            items: vec![item(
                1,
                "<script>alert('x')</script> & \"q\"",
                OverlayKind::Text,
            )],
        }
        .to_html("a<b>.pdf");
        assert!(
            !html.contains("<script>alert"),
            "raw markup leaked into the page"
        );
        assert!(html.contains("&lt;script&gt;"), "text was not escaped");
        assert!(html.contains("&amp;"), "ampersand was not escaped");
        assert!(html.contains("a&lt;b&gt;.pdf"), "title was not escaped");
    }

    #[test]
    fn links_each_box_to_its_sidebar_row() {
        let html = preview().to_html("demo.pdf");
        for i in 0..2 {
            assert!(
                html.contains(&format!("data-i=\"{i}\"")),
                "missing link {i}"
            );
        }
        assert!(html.contains("box[data-i="), "no hover wiring");
    }

    #[test]
    fn an_image_under_a_page_backdrop_is_only_outlined() {
        // Drawing the extracted image over the rendered page would double it up.
        let mut p = preview();
        p.items.push(OverlayItem {
            page_no: 1,
            bbox: [0.0, 0.0, 612.0, 792.0],
            kind: OverlayKind::Image,
            label: "picture".to_string(),
            text: String::new(),
            image_uri: Some("doc_images/pg1_1.png".to_string()),
        });
        let html = p.to_html("demo.pdf");
        assert!(html.contains("class=\"backdrop\""));
        assert!(
            !html.contains("<img class=\"box kind-image\""),
            "image was drawn again over the backdrop"
        );
        assert!(
            html.contains("class=\"box kind-image\""),
            "image box is missing"
        );
    }

    #[test]
    fn without_a_rendered_backdrop_the_extracted_image_is_shown() {
        let mut p = preview();
        p.pages[0].backdrop = None;
        p.items.push(OverlayItem {
            page_no: 1,
            bbox: [0.0, 0.0, 612.0, 792.0],
            kind: OverlayKind::Image,
            label: "picture".to_string(),
            text: String::new(),
            image_uri: Some("doc_images/pg1_1.png".to_string()),
        });
        let html = p.to_html("demo.pdf");
        assert!(html.contains("<img class=\"box kind-image\""), "{html}");
        assert!(html.contains("text reconstructed"), "fallback note missing");
    }

    #[test]
    fn reports_counts_in_the_header() {
        let html = preview().to_html("demo.pdf");
        assert!(
            html.contains("1 page · 1 text · 1 OCR · 0 figure · 0 image · 0 block"),
            "{html}"
        );
    }

    #[test]
    fn trailing_page_number_is_read_from_the_file_name() {
        assert_eq!(trailing_number(Path::new("/tmp/x/page-03.png")), Some(3));
        assert_eq!(trailing_number(Path::new("/tmp/x/page-1.png")), Some(1));
        assert_eq!(trailing_number(Path::new("/tmp/x/page.png")), None);
    }
}
