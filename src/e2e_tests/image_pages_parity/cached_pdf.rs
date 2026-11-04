//! The generated PDF is immutable for a test's lifetime. Render each requested page once per
//! side; every session still opens the real PDF plugin and every key still runs in the host/app.

use konoma_plugin::prelude::*;
use konoma_plugin::view::{ImageView, Layout};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

type Pages = Arc<Mutex<BTreeMap<(std::path::PathBuf, usize, bool), ImageView>>>;

#[derive(Default)]
pub(super) struct CachedPdf {
    pages: Pages,
}

impl CachedPdf {
    pub(super) fn rendered(&self) -> usize {
        self.pages.lock().unwrap().len()
    }
}

impl Plugin for CachedPdf {
    fn manifest(&self) -> Manifest {
        konoma_media::PdfPlugin.manifest()
    }

    fn open(&self, input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let Input::File { path, .. } = &input else {
            panic!("a PDF file");
        };
        let path = path.clone();
        Ok(Box::new(CachedSession {
            inner: konoma_media::PdfPlugin.open(input, cx)?,
            pages: self.pages.clone(),
            path,
        }))
    }
}

struct CachedSession {
    inner: Box<dyn Session>,
    pages: Pages,
    path: std::path::PathBuf,
}

impl Session for CachedSession {
    fn layout(&self) -> Layout {
        self.inner.layout()
    }

    fn view(&mut self, req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
        if cx.cancel.is_cancelled() {
            return self.inner.view(req, cx);
        }
        let key = (self.path.clone(), req.page.unwrap_or(0), cx.external);
        let mut pages = self.pages.lock().unwrap();
        if let Some(image) = pages.get(&key) {
            return Ok(View::Image(image.clone()));
        }
        let view = self.inner.view(req, cx)?;
        let View::Image(image) = &view else {
            panic!("PDF page is an image");
        };
        pages.insert(key, image.clone());
        Ok(view)
    }
}

#[test]
fn cached_pages_match_fresh_pixels_and_are_scoped_by_file() {
    let dir = super::sandbox("cached-pdf-pages");
    super::make_pdf(&dir, "three.pdf", 3);
    super::make_pdf(&dir, "one.pdf", 1);
    let cached = CachedPdf::default();
    for (file, page) in [
        ("three.pdf", 2),
        ("three.pdf", 0),
        ("one.pdf", 0),
        ("three.pdf", 1),
        ("three.pdf", 2),
    ] {
        let input = || Input::File {
            path: dir.join(file),
            hint: None,
        };
        let mut direct = konoma_media::PdfPlugin
            .open(input(), &OpenCx::bare())
            .unwrap();
        let mut memo = cached.open(input(), &OpenCx::bare()).unwrap();
        let req = ViewReq::new(60, 24).page(page);
        let pixels = |view: View| {
            let View::Image(ImageView {
                data:
                    konoma_plugin::view::ImageData::Rgba {
                        width,
                        height,
                        pixels,
                    },
                ..
            }) = view
            else {
                panic!("RGBA page");
            };
            (width, height, pixels)
        };
        assert!(
            pixels(direct.view(&req, &ViewCx::bare()).unwrap())
                == pixels(memo.view(&req, &ViewCx::bare()).unwrap()),
            "cached pixels differ: {file} page {page}"
        );
    }
    assert_eq!(cached.rendered(), 4);
}
