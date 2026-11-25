//! The frame layer: length prefix, size limit before reading, hostile bodies.

use std::io::Cursor;

use konoma_plugin::proc::frame::{self, FrameError, MAX_FRAME};
use konoma_plugin::proc::proto::*;
use konoma_plugin::proc::wire::*;

/// A reader that counts what is taken from it.
struct Counting<'a> {
    inner: Cursor<&'a [u8]>,
    taken: usize,
}

impl std::io::Read for Counting<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.taken += n;
        Ok(n)
    }
}

#[test]
fn a_message_round_trips_over_a_stream() {
    let msg = ToChild::Close { sid: 7 };
    let mut buf = Vec::new();
    frame::send(&mut buf, &msg).unwrap();
    assert_eq!(&buf[..4], &((buf.len() - 4) as u32).to_be_bytes());
    let mut r = Cursor::new(buf);
    assert_eq!(frame::recv::<_, ToChild>(&mut r).unwrap(), Some(msg));
    assert!(
        frame::recv::<_, ToChild>(&mut r).unwrap().is_none(),
        "clean end"
    );
}

#[test]
fn several_frames_in_a_row() {
    let mut buf = Vec::new();
    for sid in 0..50 {
        frame::send(&mut buf, &ToChild::Close { sid }).unwrap();
    }
    let mut r = Cursor::new(buf);
    for sid in 0..50 {
        assert_eq!(
            frame::recv::<_, ToChild>(&mut r).unwrap(),
            Some(ToChild::Close { sid })
        );
    }
    assert!(frame::recv::<_, ToChild>(&mut r).unwrap().is_none());
}

#[test]
fn a_huge_length_prefix_is_refused_before_anything_is_read() {
    for len in [u32::MAX, MAX_FRAME as u32 + 1, 0x8000_0000] {
        let mut bytes = len.to_be_bytes().to_vec();
        bytes.extend_from_slice(&[0u8; 64]);
        let mut r = Counting {
            inner: Cursor::new(&bytes),
            taken: 0,
        };
        match frame::read_frame(&mut r) {
            Err(FrameError::TooLarge(n)) => assert_eq!(n, len as u64),
            other => panic!("{len}: {other:?}"),
        }
        assert_eq!(r.taken, 4, "{len}: only the prefix was taken, not the body");
    }
}

#[test]
fn the_limit_itself_is_allowed_through_the_prefix_check() {
    // Exactly MAX_FRAME is announced: the prefix passes, the (absent) body is what is missing.
    let bytes = (MAX_FRAME as u32).to_be_bytes().to_vec();
    let mut r = Cursor::new(bytes);
    assert!(matches!(
        frame::read_frame(&mut r),
        Err(FrameError::Truncated)
    ));
}

#[test]
fn a_body_that_is_cut_short_is_truncated() {
    let mut bytes = 100u32.to_be_bytes().to_vec();
    bytes.extend_from_slice(&[1, 2, 3]);
    assert!(matches!(
        frame::read_frame(&mut Cursor::new(bytes)),
        Err(FrameError::Truncated)
    ));
}

#[test]
fn a_prefix_that_is_cut_short_is_truncated_and_nothing_is_a_clean_end() {
    for n in 1..4 {
        let bytes = vec![0u8; n];
        assert!(
            matches!(
                frame::read_frame(&mut Cursor::new(bytes)),
                Err(FrameError::Truncated)
            ),
            "{n} bytes of prefix"
        );
    }
    assert!(frame::read_frame(&mut Cursor::new(Vec::<u8>::new()))
        .unwrap()
        .is_none());
}

#[test]
fn an_empty_body_is_a_frame_but_not_a_message() {
    let mut r = Cursor::new(0u32.to_be_bytes().to_vec());
    assert_eq!(frame::read_frame(&mut r).unwrap(), Some(vec![]));
    let mut r = Cursor::new(0u32.to_be_bytes().to_vec());
    assert!(matches!(
        frame::recv::<_, ToChild>(&mut r),
        Err(FrameError::Codec(_))
    ));
}

#[test]
fn garbage_and_other_shapes_are_codec_errors_not_panics() {
    let bodies: Vec<Vec<u8>> = vec![
        vec![0xc1], // a reserved byte
        vec![0xff; 40],
        vec![0x91],                         // an array of one with nothing in it
        vec![0xdd, 0xff, 0xff, 0xff, 0xff], // an array announcing 4 billion items
        vec![0xdf, 0xff, 0xff, 0xff, 0xff], // a map announcing 4 billion pairs
        vec![0xc6, 0xff, 0xff, 0xff, 0xff], // a bin announcing 4 GiB
        vec![0xdb, 0xff, 0xff, 0xff, 0xff], // a str announcing 4 GiB
        frame::encode(&("a", 1, 2.5)).unwrap(),
        frame::encode(&"Shutdown2").unwrap(),
    ];
    for (i, b) in bodies.iter().enumerate() {
        assert!(
            matches!(frame::decode::<ToChild>(b), Err(FrameError::Codec(_))),
            "case {i}: {:?}",
            frame::decode::<ToChild>(b).map(|_| ())
        );
    }
}

#[test]
fn trailing_bytes_after_a_message_are_refused() {
    let mut body = frame::encode(&ToChild::Shutdown).unwrap();
    assert_eq!(frame::decode::<ToChild>(&body).unwrap(), ToChild::Shutdown);
    body.push(0xc0);
    assert!(matches!(
        frame::decode::<ToChild>(&body),
        Err(FrameError::Codec(_))
    ));
}

#[test]
fn a_deeply_nested_body_is_an_error_and_does_not_overflow_the_stack() {
    for depth in [200usize, 10_000, 1_000_000] {
        let body = vec![0x91u8; depth]; // [[[[ ... with no end
        assert!(frame::decode::<WValue>(&body).is_err(), "depth {depth}");
        let mut closed = vec![0x91u8; depth];
        closed.push(0xc0);
        assert!(
            frame::decode::<WValue>(&closed).is_err(),
            "depth {depth} closed"
        );
        assert!(
            frame::decode::<ToChild>(&closed).is_err(),
            "depth {depth} as ToChild"
        );
    }
    // Within the limit it decodes.
    let mut v = WValue::Bool(true);
    for _ in 0..20 {
        v = WValue::List(vec![v]);
    }
    let body = frame::encode(&v).unwrap();
    assert_eq!(frame::decode::<WValue>(&body).unwrap(), v);
}

#[test]
fn writing_a_body_over_the_limit_is_refused_and_writes_nothing() {
    let body = vec![0u8; MAX_FRAME + 1];
    let mut out = Vec::new();
    assert!(matches!(
        frame::write_frame(&mut out, &body),
        Err(FrameError::TooLarge(_))
    ));
    assert!(out.is_empty());
    let body = vec![0u8; 1000];
    frame::write_frame(&mut out, &body).unwrap();
    assert_eq!(out.len(), 1004);
}

#[test]
fn a_frame_can_arrive_in_dribbles() {
    struct OneByte(Cursor<Vec<u8>>);
    impl std::io::Read for OneByte {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = buf.len().min(1);
            self.0.read(&mut buf[..n])
        }
    }
    let mut buf = Vec::new();
    frame::send(&mut buf, &ToChild::Cancel { rid: 99 }).unwrap();
    let mut r = OneByte(Cursor::new(buf));
    assert_eq!(
        frame::recv::<_, ToChild>(&mut r).unwrap(),
        Some(ToChild::Cancel { rid: 99 })
    );
}
