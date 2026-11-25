use konoma_plugin::prelude::*;

pub struct Hexdump;

impl Plugin for Hexdump {
    fn manifest(&self) -> Manifest {
        Manifest::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
            .name(t!(en: "Hex dump", ja: "16進ダンプ"))
            .bind(Bind::files(["*.bin"]))
            .shows(ViewKind::Text)
            .action(
                Act::new("width")
                    .key("w")
                    .label(t!(en: "width", ja: "幅"))
                    .help(t!(en: "Switch 8/16 bytes per row", ja: "1行のバイト数を8/16に切り替え"))
                    .footer(1),
            )
    }

    fn open(&self, input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let Input::File { path, .. } = input else {
            return Err(Failure::unsupported(
                t!(en: "Expected a file", ja: "ファイルが必要です"),
            ));
        };
        Ok(Box::new(HexSession {
            bytes: std::fs::read(path)?,
            width: 16,
        }))
    }
}

struct HexSession {
    bytes: Vec<u8>,
    width: usize,
}

impl Session for HexSession {
    fn view(&mut self, _req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
        let lines = self
            .bytes
            .chunks(self.width)
            .enumerate()
            .map(|(row, bytes)| {
                let hex = bytes
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                let ascii: String = bytes
                    .iter()
                    .map(|&b| {
                        if b.is_ascii_graphic() || b == b' ' {
                            b as char
                        } else {
                            '.'
                        }
                    })
                    .collect();
                format!(
                    "{:08x}  {hex:<pad$}  |{ascii}|",
                    row * self.width,
                    pad = self.width * 3 - 1
                )
            })
            .collect::<Vec<_>>();
        Ok(View::text(lines))
    }

    fn act(&mut self, action: &str, _cx: &ActCx) -> Result<Outcome, Failure> {
        if action != "width" {
            return Ok(Outcome::Unhandled);
        }
        self.width = if self.width == 16 { 8 } else { 16 };
        Ok(Outcome::Redraw)
    }
}

pub fn plugins() -> Vec<Box<dyn Plugin>> {
    vec![Box::new(Hexdump)]
}
