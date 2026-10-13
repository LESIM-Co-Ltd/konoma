//! Tests added after the second stage-2 mutation audit: a read that is cancelled **part of the
//! way through** (after the open's own look passed) ends as a cancelled open, never as a broken
//! file, for the CSV plugin and for every archive kind. Driven through the plugins' `open`, as the
//! host does.

use std::io::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use konoma_plugin::{Cancel, FailureKind, Input, OpenCx, Plugin, Settings};
use konoma_table::{ArchivePlugin, CsvPlugin};
use konoma_test_support::unique_tmp;

/// A cancel check that says no to the first look (the open's own, before it reads) and yes to
/// every later one (the reader's, while it works).
fn cancel_after_the_first_look() -> Cancel {
    let looks = Arc::new(AtomicUsize::new(0));
    Cancel::from_fn(move || looks.fetch_add(1, Ordering::SeqCst) >= 1)
}

fn open_with(plugin: &dyn Plugin, path: &Path, cancel: Cancel) -> FailureKind {
    let settings = Settings::new();
    let cx = OpenCx::new(&settings, "en").with_cancel(cancel);
    plugin
        .open(
            Input::File {
                path: path.to_path_buf(),
                hint: None,
            },
            &cx,
        )
        .err()
        .expect("a cancelled open")
        .kind
}

#[test]
fn a_csv_cancelled_while_it_is_read_is_a_cancelled_open() {
    let dir = unique_tmp("konoma_table_f8b_csv");
    std::fs::create_dir_all(&dir).unwrap();
    for name in ["t.csv", "t.tsv"] {
        let p = dir.join(name);
        std::fs::write(&p, "a,b\n1,2\n3,4\n").unwrap();
        assert_eq!(
            open_with(&CsvPlugin, &p, cancel_after_the_first_look()),
            FailureKind::Cancelled,
            "{name}"
        );
        // The same file reads when nothing cancels.
        let settings = Settings::new();
        let cx = OpenCx::new(&settings, "en");
        assert!(CsvPlugin
            .open(
                Input::File {
                    path: p,
                    hint: None
                },
                &cx
            )
            .is_ok());
    }
}

fn zip_of(path: &Path) {
    let mut zw = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zw.start_file("one.txt", opts).unwrap();
    zw.write_all(b"hello").unwrap();
    zw.finish().unwrap();
}

fn tar_of(path: &Path, gz: bool) {
    let mut bytes = Vec::new();
    {
        let mut b = tar::Builder::new(&mut bytes);
        let data = b"hello";
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        b.append_data(&mut h, "one.txt", &data[..]).unwrap();
        b.finish().unwrap();
    }
    if gz {
        let f = std::fs::File::create(path).unwrap();
        let mut enc = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
        enc.write_all(&bytes).unwrap();
        enc.finish().unwrap();
    } else {
        std::fs::write(path, bytes).unwrap();
    }
}

#[test]
fn an_archive_cancelled_while_it_is_listed_is_a_cancelled_open() {
    let dir = unique_tmp("konoma_table_f8b_archive");
    std::fs::create_dir_all(&dir).unwrap();
    let zip = dir.join("t.zip");
    zip_of(&zip);
    let tar = dir.join("t.tar");
    tar_of(&tar, false);
    let tgz = dir.join("t.tar.gz");
    tar_of(&tgz, true);
    for p in [zip, tar, tgz] {
        assert_eq!(
            open_with(&ArchivePlugin, &p, cancel_after_the_first_look()),
            FailureKind::Cancelled,
            "{}",
            p.display()
        );
        // And they list when nothing cancels.
        let settings = Settings::new();
        let cx = OpenCx::new(&settings, "en");
        let opened = ArchivePlugin.open(
            Input::File {
                path: p.clone(),
                hint: None,
            },
            &cx,
        );
        assert!(opened.is_ok(), "{}", p.display());
    }
}
