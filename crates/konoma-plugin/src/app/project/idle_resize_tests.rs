//! Resize publication through the real project Preview and its still-picture owner.
use super::*;
use ratatui::{
    backend::{Backend, ClearType, WindowSize},
    layout::{Position, Size},
    style::Color,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};

struct SharedBackend(Arc<Mutex<TestBackend>>);
impl Backend for SharedBackend {
    type Error = std::convert::Infallible;
    fn draw<'a, I>(&mut self, cells: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
    {
        self.0.lock().unwrap().draw(cells)
    }
    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        Ok((0, 0).into())
    }
    fn set_cursor_position<P: Into<Position>>(&mut self, _: P) -> Result<(), Self::Error> {
        Ok(())
    }
    fn clear(&mut self) -> Result<(), Self::Error> {
        self.0.lock().unwrap().clear()
    }
    fn clear_region(&mut self, region: ClearType) -> Result<(), Self::Error> {
        self.0.lock().unwrap().clear_region(region)
    }
    fn scroll_region_up(
        &mut self,
        region: std::ops::Range<u16>,
        line_count: u16,
    ) -> Result<(), Self::Error> {
        self.0.lock().unwrap().scroll_region_up(region, line_count)
    }
    fn scroll_region_down(
        &mut self,
        region: std::ops::Range<u16>,
        line_count: u16,
    ) -> Result<(), Self::Error> {
        self.0
            .lock()
            .unwrap()
            .scroll_region_down(region, line_count)
    }
    fn size(&self) -> Result<Size, Self::Error> {
        self.0.lock().unwrap().size()
    }
    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        self.0.lock().unwrap().window_size()
    }
    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

struct ImagePlugin;
impl Plugin for ImagePlugin {
    fn manifest(&self) -> Manifest {
        Manifest::new("resize-image", "0")
            .bind(Bind::files(["*.png"]))
            .shows(ViewKind::Image)
    }
    fn open(&self, input: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let Input::File { path, .. } = input else {
            unreachable!()
        };
        Ok(Box::new(ImageSession(path)))
    }
}
struct ImageSession(PathBuf);
impl Session for ImageSession {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        Ok(View::Image(crate::view::ImageView::new(
            crate::view::ImageData::File(self.0.clone()),
        )))
    }
}

#[test]
fn idle_fence_lands_the_still_picture_after_a_terminal_resize() {
    let dir = konoma_test_support::unique_tmp("project_idle_resize");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("project.marker"), "").unwrap();
    image::RgbaImage::from_pixel(800, 800, image::Rgba([10, 190, 30, 255]))
        .save(dir.join("image.png"))
        .unwrap();
    let mut prepared = super::super::super::prepare::prepare_projects_with_previews(
        vec![Arc::new(Project)],
        vec![Arc::new(ImagePlugin)],
        &dir,
        TermCaps::new(),
        (
            SharedConfig::parse("[ui]\nlang='en'\ndiff_view='preview'\n").unwrap(),
            None,
        ),
        Arc::new(Services),
    )
    .unwrap_or_else(|e| panic!("{}", e.error));
    let path = prepared.open_dir.join("image.png");
    prepared.diff = Some(
        crate::app::diff_screen::DiffScreenState::open_for_project(
            FileDiffRequest {
                path: path.clone(),
                root: prepared.open_dir.clone(),
                info: prepared.service_info.values().next().unwrap().clone(),
            },
            prepared.services.clone(),
            &prepared.config,
            prepared.settings.diff_layout,
            None,
            prepared.file_previews.clone(),
        )
        .unwrap(),
    );
    prepared.diff_return = prepared.screen.take();
    let mut pictures = Pictures::new(
        ratatui_image::picker::Picker::halfblocks(),
        "resize-image",
        &path,
        800,
    )
    .with_out(Box::new(Vec::<u8>::new()));
    // Construct the still owner before installing a held resize channel. Entering Preview
    // resets its file pose, but retains the owner and its real resize worker.
    pictures.submit_image(
        &crate::view::ImageView::new(crate::view::ImageData::File(path)),
        false,
    );
    pictures.wait_still_decode_for_test();
    let (go, waits) = pictures.hold_still_resizes_for_test();
    let released = Arc::new(AtomicUsize::new(0));
    let releases = released.clone();
    std::thread::spawn(move || {
        for _ in 0..2 {
            waits
                .recv_timeout(Duration::from_secs(30))
                .expect("the fence must wait for resize publication");
            releases.fetch_add(1, Ordering::SeqCst);
            go.send(()).expect("held resize worker is alive");
        }
    });
    let backend = Arc::new(Mutex::new(TestBackend::new(40, 12)));
    let mut terminal = Terminal::new(SharedBackend(backend.clone())).unwrap();
    let mut stages = 0;
    let mut first_pixels = 0;
    super::super::super::run_project_fenced_for_test(
        &mut terminal,
        &mut prepared,
        &ProjectLoopCx::default(),
        Some(pictures),
        |snapshot, frame| {
            assert!(snapshot.is_idle(), "{snapshot:?}");
            let pixels = frame
                .content
                .iter()
                .filter(|cell| {
                    cell.fg == Color::Rgb(10, 190, 30) || cell.bg == Color::Rgb(10, 190, 30)
                })
                .count();
            assert!(
                pixels > 0,
                "stage {stages}: a fenced frame must contain the landed still picture"
            );
            assert_eq!(
                released.load(Ordering::SeqCst),
                stages + 1,
                "each fenced picture must wait for resize publication"
            );
            stages += 1;
            if stages == 1 {
                assert_eq!(frame.area, Rect::new(0, 0, 40, 12));
                first_pixels = pixels;
                backend.lock().unwrap().resize(64, 20);
                Next::Event(AppEvent::Resize)
            } else {
                assert_eq!(frame.area, Rect::new(0, 0, 64, 20));
                assert!(
                    pixels > first_pixels,
                    "the terminal resize must land the larger picture before returning"
                );
                Next::End
            }
        },
    )
    .unwrap();
    assert_eq!(stages, 2, "one initial step and one resized step");
    assert!(
        prepared.file_preview.is_some(),
        "the resized frame is still a project Preview"
    );
}
