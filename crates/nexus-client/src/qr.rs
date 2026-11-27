//! A QR code in a terminal, in half blocks: what `nexus devices pair` and `nexus-host login --pair` print.

/// The QR code in half blocks, dark on light whatever the terminal's colours, with a two-module quiet zone. None when
/// the text does not fit a QR code.
pub fn lines(text: &str) -> Option<Vec<String>> {
    use qrcodegen::{QrCode, QrCodeEcc};
    let qr = QrCode::encode_text(text, QrCodeEcc::Medium).ok()?;
    let quiet = 2;
    let size = qr.size();
    let dark = |x: i32, y: i32| (0..size).contains(&x) && (0..size).contains(&y) && qr.get_module(x, y);
    let mut lines = Vec::new();
    let mut y = -quiet;
    while y < size + quiet {
        // Black on white (256-colour 16 and 231); the glyph draws the dark modules in the foreground.
        let mut line = String::from("\x1b[38;5;16;48;5;231m");
        for x in -quiet..size + quiet {
            line.push(match (dark(x, y), dark(x, y + 1)) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        line.push_str("\x1b[0m");
        lines.push(line);
        y += 2;
    }
    Some(lines)
}
