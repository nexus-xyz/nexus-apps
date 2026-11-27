//! The recording's video and narration, through Apple's frameworks: keyframes decoded from `recording.mov` with
//! AVFoundation (`AVAssetImageGenerator`, fitted to 1600 px) and drawn into RGB pixels with Core Graphics, and the
//! narration transcribed by Speech on this Mac (`requiresOnDeviceRecognition`: the voice never leaves it). macOS only.
//!
//! Unsafe: the objc2 bindings mark these framework calls `unsafe`; each takes valid, retained objects made here, and
//! the bitmap is read only within the bytes Core Graphics allocated for it.
#![allow(unsafe_code)]

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::{AnyThread, sel};
use objc2_av_foundation::{AVAssetImageGenerator, AVURLAsset};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapContextGetBytesPerRow, CGBitmapContextGetData, CGColorSpace, CGContext, CGImage,
    CGImageAlphaInfo, CGImageByteOrderInfo, kCGColorSpaceSRGB,
};
use objc2_core_media::CMTime;
use objc2_foundation::{NSError, NSLocale, NSObjectProtocol, NSOperationQueue, NSString, NSURL};
use objc2_speech::{
    SFSpeechRecognitionResult, SFSpeechRecognizer, SFSpeechRecognizerAuthorizationStatus, SFSpeechURLRecognitionRequest,
};

use super::model::{Transcript, TranscriptStatus};
use super::sheet::Rgb;
use super::transcript::{Word, phrases};

/// The keyframes' longest side, in pixels: what an agent's image input keeps.
pub const FRAME_SIZE: f64 = 1600.0;

fn file_url(path: &Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(&path.display().to_string()))
}

fn describe(error: &NSError) -> String {
    error.localizedDescription().to_string()
}

pub struct Video {
    asset: Retained<AVURLAsset>,
    generator: Retained<AVAssetImageGenerator>,
}

impl Video {
    pub fn open(path: &Path, size: f64) -> Result<Video, String> {
        if !path.exists() {
            return Err(format!("{} does not exist", path.display()));
        }
        // SAFETY: a file URL made here; no options.
        let asset = unsafe { AVURLAsset::URLAssetWithURL_options(&file_url(path), None) };
        // SAFETY: the asset made above; then plain property setters.
        let generator = unsafe { AVAssetImageGenerator::assetImageGeneratorWithAsset(&asset) };
        unsafe {
            generator.setAppliesPreferredTrackTransform(true);
            if size > 0.0 {
                generator.setMaximumSize(CGSize::new(size, size));
            }
            // The exact moment, not the nearest keyframe of the encoding: ink is on screen for a few seconds only.
            let tolerance = CMTime::with_seconds(0.05, 600);
            generator.setRequestedTimeToleranceBefore(tolerance);
            generator.setRequestedTimeToleranceAfter(tolerance);
        }
        Ok(Video { asset, generator })
    }

    /// Seconds.
    pub fn duration(&self) -> f64 {
        // SAFETY: a property read on the asset; `seconds` reads a plain struct.
        let seconds = unsafe { self.asset.duration().seconds() };
        if seconds.is_finite() { seconds.max(0.0) } else { 0.0 }
    }

    /// The frame at `t` seconds, as RGB pixels.
    pub fn frame(&self, t: f64) -> Result<Rgb, String> {
        // SAFETY: a valid time; `actual_time` may be null. The method is deprecated in favour of a callback version
        // that needs macOS 13; the app supports 12.
        #[allow(deprecated)]
        let image = unsafe {
            self.generator
                .copyCGImageAtTime_actualTime_error(CMTime::with_seconds(t.max(0.0), 600), std::ptr::null_mut())
        }
        .map_err(|error| format!("no frame at {t:.2} s: {}", describe(&error)))?;
        pixels(&image)
    }
}

/// Draw `image` into an RGBX bitmap (sRGB) and keep the RGB.
fn pixels(image: &CGImage) -> Result<Rgb, String> {
    let (width, height) = (CGImage::width(Some(image)), CGImage::height(Some(image)));
    if width == 0 || height == 0 {
        return Err("an empty frame".into());
    }
    // SAFETY: a constant Core Graphics exports.
    let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })).ok_or("no sRGB colour space")?;
    let info = CGImageAlphaInfo::NoneSkipLast.0 | CGImageByteOrderInfo::Order32Big.0;
    // SAFETY: a null `data` lets Core Graphics allocate (and own) the bitmap; the sizes are the image's.
    let context =
        unsafe { CGBitmapContextCreate(std::ptr::null_mut(), width, height, 8, width * 4, Some(&space), info) }
            .ok_or("could not make a bitmap")?;
    let rect = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(width as f64, height as f64));
    CGContext::draw_image(Some(&context), rect, Some(image));
    let data = CGBitmapContextGetData(Some(&context)).cast::<u8>();
    let row = CGBitmapContextGetBytesPerRow(Some(&context));
    if data.is_null() || row < width * 4 {
        return Err("the bitmap has no pixels".into());
    }
    // SAFETY: Core Graphics allocated `row * height` bytes at `data`, alive as long as `context` (until this returns).
    let bytes = unsafe { std::slice::from_raw_parts(data, row * height) };
    let mut out = Vec::with_capacity(width * height * 3);
    for y in 0..height {
        for pixel in bytes[y * row..y * row + width * 4].as_chunks::<4>().0 {
            out.extend_from_slice(&pixel[..3]);
        }
    }
    Ok(Rgb {
        width,
        height,
        pixels: out,
    })
}

/// The narration's audio as an M4A beside the video, for the transcriber (`avconvert`, part of macOS). `None` when it
/// cannot be made; the transcriber then reads the video itself.
fn narration(video: &Path, dir: &Path) -> Option<std::path::PathBuf> {
    let out = dir.join("narration.m4a");
    let status = std::process::Command::new("/usr/bin/avconvert")
        .args(["--preset", "PresetAppleM4A", "--replace", "--source"])
        .arg(video)
        .arg("--output")
        .arg(&out)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;
    (status.success() && out.exists()).then_some(out)
}

fn unavailable(note: &str) -> Transcript {
    Transcript {
        status: TranscriptStatus::Unavailable,
        note: Some(note.into()),
        ..Transcript::default()
    }
}

/// Whether this process was opened as an app (from Finder, the Dock or `open`): inside a `.app`, with launchd as its
/// parent. macOS holds a process started any other way (`cargo run`, a terminal) responsible to
/// the app that started it, and ends it (SIGABRT, namespace TCC) when it asks for speech recognition and that app's
/// Info.plist has no `NSSpeechRecognitionUsageDescription`. A debug build opened as a `.app` asks safely.
fn opened_as_app() -> bool {
    let in_bundle = std::env::current_exe().is_ok_and(|exe| {
        let mut dirs = exe.ancestors().skip(1);
        dirs.next().is_some_and(|dir| dir.ends_with("MacOS"))
            && dirs.next().is_some_and(|dir| dir.ends_with("Contents"))
            && dirs
                .next()
                .is_some_and(|dir| dir.extension().is_some_and(|extension| extension == "app"))
    });
    in_bundle && std::os::unix::process::parent_id() == 1
}

/// Whether Nexus may use speech recognition, or why not. Asks the person the first time when `ask` (the app; never
/// from the command line, where the terminal would be asked instead) and the process was opened as an app.
fn authorized(ask: bool) -> Result<(), &'static str> {
    const OFF: &str = "speech recognition is off for Nexus (System Settings › Privacy & Security › Speech Recognition)";
    const OUTSIDE: &str = "this Nexus was not opened as Nexus.app (a development build, or started from a terminal) and \
                           cannot ask to use speech recognition; record from Nexus.app for a transcript";
    // SAFETY: a class method with no arguments.
    let status = unsafe { SFSpeechRecognizer::authorizationStatus() };
    if status == SFSpeechRecognizerAuthorizationStatus::Authorized {
        return Ok(());
    }
    // Here the status is the starting app's, not Nexus's: neither ask nor say that it is off.
    if !opened_as_app() {
        return Err(OUTSIDE);
    }
    if status != SFSpeechRecognizerAuthorizationStatus::NotDetermined {
        return Err(OFF);
    }
    if !ask {
        return Err("Nexus has not been allowed to transcribe yet; it asks when a recording made in the app finishes");
    }
    let (send, receive) = mpsc::channel();
    let handler = RcBlock::new(move |status: SFSpeechRecognizerAuthorizationStatus| {
        let _ = send.send(status);
    });
    // SAFETY: the block lives until the call returns and is copied by the framework.
    unsafe { SFSpeechRecognizer::requestAuthorization(&handler) };
    // The person answers the system's prompt.
    match receive.recv_timeout(Duration::from_secs(120)) {
        Ok(status) if status == SFSpeechRecognizerAuthorizationStatus::Authorized => Ok(()),
        _ => Err(OFF),
    }
}

enum Heard {
    Words(Vec<Word>),
    Nothing,
    Failed(String),
}

/// Transcribe the narration of `video` (in `dir`) on this Mac.
pub fn transcribe(video: &Path, dir: &Path, ask: bool) -> Transcript {
    if let Err(why) = authorized(ask) {
        return unavailable(why);
    }
    // SAFETY: plain initialisers and property reads on objects made here.
    let Some(recognizer) = (unsafe { SFSpeechRecognizer::init(SFSpeechRecognizer::alloc()) }) else {
        return unavailable("this Mac has no speech recognizer for its language");
    };
    let locale = unsafe { recognizer.locale() };
    let locale_id = locale_identifier(&locale);
    if !unsafe { recognizer.supportsOnDeviceRecognition() } {
        return Transcript {
            locale: Some(locale_id.clone()),
            ..unavailable(&format!(
                "speech recognition for {locale_id} does not run on this Mac, and Nexus does not send your voice off it"
            ))
        };
    }
    let audio = narration(video, dir);
    let heard = recognize(&recognizer, audio.as_deref().unwrap_or(video));
    if let Some(audio) = audio {
        let _ = std::fs::remove_file(audio);
    }
    match heard {
        Heard::Words(words) if !words.is_empty() => Transcript {
            status: TranscriptStatus::Ok,
            note: None,
            locale: Some(locale_id),
            phrases: phrases(&words),
        },
        Heard::Words(_) | Heard::Nothing => Transcript {
            status: TranscriptStatus::Silent,
            locale: Some(locale_id),
            ..Transcript::default()
        },
        Heard::Failed(error) => Transcript {
            status: TranscriptStatus::Failed,
            note: Some(error),
            locale: Some(locale_id),
            phrases: Vec::new(),
        },
    }
}

fn locale_identifier(locale: &NSLocale) -> String {
    locale.localeIdentifier().to_string().replace('_', "-")
}

fn recognize(recognizer: &SFSpeechRecognizer, audio: &Path) -> Heard {
    let (send, receive) = mpsc::channel::<Heard>();
    // SAFETY: objects made here; the handler is copied by the framework and runs on `queue`, not the main thread.
    let task = unsafe {
        let queue = NSOperationQueue::new();
        recognizer.setQueue(&queue);
        let request =
            SFSpeechURLRecognitionRequest::initWithURL(SFSpeechURLRecognitionRequest::alloc(), &file_url(audio));
        request.setRequiresOnDeviceRecognition(true);
        request.setShouldReportPartialResults(false);
        // macOS 13 and later.
        if request.respondsToSelector(sel!(setAddsPunctuation:)) {
            request.setAddsPunctuation(true);
        }
        let handler = RcBlock::new(move |result: *mut SFSpeechRecognitionResult, error: *mut NSError| {
            if let Some(error) = error.as_ref() {
                // "No speech detected" (kAFAssistantErrorDomain 1110): a silent recording.
                let heard = if error.code() == 1110 {
                    Heard::Nothing
                } else {
                    Heard::Failed(describe(error))
                };
                let _ = send.send(heard);
                return;
            }
            let Some(result) = result.as_ref() else {
                return;
            };
            if !result.isFinal() {
                return;
            }
            let segments = result.bestTranscription().segments();
            let words = segments
                .iter()
                .map(|segment| Word {
                    text: segment.substring().to_string(),
                    start: segment.timestamp(),
                    duration: segment.duration(),
                })
                .collect();
            let _ = send.send(Heard::Words(words));
        });
        recognizer.recognitionTaskWithRequest_resultHandler(&request, &handler)
    };
    let heard = receive
        .recv_timeout(Duration::from_secs(600))
        .unwrap_or_else(|_| Heard::Failed("the transcriber did not answer in ten minutes".into()));
    // SAFETY: cancelling a finished task does nothing.
    unsafe { task.cancel() };
    heard
}
