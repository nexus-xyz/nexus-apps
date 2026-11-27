use super::*;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc,
};
use std::time::Duration;
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "nexus-guarded-editor-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("fixture clock follows the Unix epoch")
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&root).expect("isolated editor root");
        std::fs::write(root.join("file with spaces.rs"), "text").expect("actual contained source file");
        Self(root)
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn request(root: &Path) -> FileRequest<'_> {
    FileRequest {
        root,
        path: "file with spaces.rs",
        line: Some(7),
        col: Some(2),
        editor: None,
        search_path: "",
    }
}
#[test]
fn authority_lost_after_command_preparation_never_reaches_real_spawn_boundary() {
    let root = Root::new();
    let path = root.0.clone();
    let live = Arc::new(AtomicBool::new(true));
    let valid = live.clone();
    let (prepared, ready) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let released = Mutex::new(released);
    let calls = AtomicUsize::new(0);
    let worker = std::thread::spawn(move || {
        launch_file(
            request(&path),
            &move || {
                if calls.fetch_add(1, Ordering::SeqCst) == 1 {
                    prepared.send(()).expect("command preparation observer is alive");
                    released
                        .lock()
                        .expect("private release receiver")
                        .recv_timeout(Duration::from_secs(3))
                        .expect("prepared command released");
                }
                valid.load(Ordering::SeqCst)
            },
            |_, _| panic!("a revoked prepared command cannot spawn"),
        )
    });
    ready
        .recv_timeout(Duration::from_secs(3))
        .expect("actual command was prepared");
    live.store(false, Ordering::SeqCst);
    release.send(()).expect("release held command guard");
    assert!(
        worker
            .join()
            .expect("guard worker completes")
            .expect_err("revoked command must be refused")
            .contains("no longer active")
    );
    let expected = open::file_inside(&root.0, "file with spaces.rs").expect("real contained path");
    launch_file(request(&root.0), &|| true, |command, what| {
        assert_eq!(what, expected.display().to_string());
        assert!(
            command.get_args().any(|arg| arg == expected.as_os_str()),
            "contained path remains one argument"
        );
        Ok(())
    })
    .expect("a fresh current presentation prepares the ordinary platform opener");
}
#[test]
fn guarded_launch_keeps_actual_path_containment_checks() {
    let root = Root::new();
    let other = Root::new();
    let outside = other.0.join("file with spaces.rs");
    let request = FileRequest {
        root: &root.0,
        path: outside.to_str().expect("UTF-8 fixture path"),
        ..request(&root.0)
    };
    assert!(launch_file(request, &|| true, |_, _| panic!("outside path must not launch")).is_err());
}

#[test]
fn final_process_spawn_gate_rejects_a_retired_view_before_attempting_any_program() {
    let error = spawn_detached_guarded(
        crate::process::command("/not-a-real-editor/never-execute"),
        "fixture",
        &|| false,
    )
    .expect_err("retired view must be denied at the final spawn boundary");
    assert!(
        error.contains("no longer active"),
        "the final authority gate must run before Command::spawn: {error}"
    );
}
