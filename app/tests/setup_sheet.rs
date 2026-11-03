//! setup sheet tests (M5 T8): event-to-state reduction and button model —
//! synthetic EnvEvents, no processes.

use app::sheets::{SetupButtons, SetupSheetState};
use engine::python::env::{EnvEvent, EnvFailure, EnvLog, EnvState, EnvStep, InstallError};

#[test]
fn events_reduce_to_sheet_state() {
    app::localize::select("en");
    let mut s = SetupSheetState::new();
    assert_eq!(s.state, EnvState::Unknown);
    s.apply(&EnvEvent::Progress {
        value: 0.1,
        step: EnvStep::CreatingVenv,
    });
    assert_eq!(
        (s.progress, s.step.as_str()),
        (0.1, "Creating the virtual environment…")
    );
    s.apply(&EnvEvent::Log(EnvLog::Text("$ uv venv".into())));
    s.apply(&EnvEvent::Log(EnvLog::Step(EnvStep::UpgradingPip)));
    assert!(s.log.contains("$ uv venv\nUpdating pip…\n"), "{}", s.log);
    s.apply(&EnvEvent::Finished(Ok(engine::python::env::EnvReady {
        build123d_version: "0.8.1".into(),
        versions: "Python 3.12.4, build123d 0.8.1".into(),
    })));
    assert_eq!(
        s.state,
        EnvState::Ready {
            build123d_version: "0.8.1".into()
        }
    );
    // no versions_display() assertion here (MINOR 7): the shared cell is
    // set by the T0 drivers, not by the sheet reducer — T0 pins it.
    let mut f = SetupSheetState::new();
    // MAJOR 1 (shape (b)): typed failures — Missing is its own sheet state
    f.apply(&EnvEvent::Finished(Err(
        engine::python::env::EnvFailure::Missing,
    )));
    assert_eq!(f.state, EnvState::Missing);
    f.apply(&EnvEvent::Finished(Err(EnvFailure::InstallFailed(
        InstallError::CommandFailed {
            command: "uv".into(),
        },
    ))));
    assert_eq!(
        f.state,
        EnvState::Failed("The command “uv” failed. Check the log for details.".into())
    );
    // CheckFailed localizes its catalog label into Failed
    f.apply(&EnvEvent::Finished(Err(EnvFailure::CheckFailed)));
    assert_eq!(
        f.state,
        EnvState::Failed(
            "The Python environment exists, but build123d or matplotlib can’t be imported.".into()
        )
    );
}

#[test]
fn button_model_matches_upstream_states() {
    assert!(matches!(
        app::sheets::buttons(&EnvState::Ready {
            build123d_version: String::new()
        }),
        SetupButtons::Done
    ));
    assert!(matches!(
        app::sheets::buttons(&EnvState::Installing),
        SetupButtons::Busy
    ));
    assert!(matches!(
        app::sheets::buttons(&EnvState::Checking),
        SetupButtons::Busy
    ));
    assert!(matches!(
        app::sheets::buttons(&EnvState::Unknown),
        SetupButtons::LaterAndInstall
    ));
    assert!(matches!(
        app::sheets::buttons(&EnvState::Missing),
        SetupButtons::LaterAndInstall
    ));
    assert!(matches!(
        app::sheets::buttons(&EnvState::Failed("x".into())),
        SetupButtons::LaterAndInstall
    ));
}

/// The app-side state entries: `check`'s kick marks Checking (upstream
/// `check()` sets `.checking` first thing, PythonEnvironment.swift:35) and
/// `install`'s kick clears the previous run — upstream install()'s
/// `state = .installing; log = ""; progress = 0` (:56-58) — so a failed
/// run's log and bar never bleed into the retry.
#[test]
fn kicks_enter_checking_and_installing() {
    let mut sheet = SetupSheetState::new();
    sheet.apply(&EnvEvent::Log(EnvLog::Text(
        "stale log from a failed run".into(),
    )));
    sheet.apply(&EnvEvent::Progress {
        value: 0.4,
        step: EnvStep::InstallFailed,
    });
    sheet.begin_checking();
    assert_eq!(sheet.state, EnvState::Checking);
    assert_eq!(
        sheet.log, "stale log from a failed run\n",
        "checking keeps the log"
    );
    sheet.begin_installing();
    assert_eq!(sheet.state, EnvState::Installing);
    assert_eq!(sheet.log, "");
    assert_eq!((sheet.progress, sheet.step.as_str()), (0.0, ""));
}

/// MINOR 8: `interactiveDismissDisabled(python.isInstalling)`
/// (PythonSetupSheet.swift:58) — dismissal is blocked while an INSTALL
/// runs only; a check and every terminal state stay dismissable.
#[test]
fn dismissal_blocked_only_while_installing() {
    assert!(app::sheets::dismiss_blocked(&EnvState::Installing));
    for state in [
        EnvState::Unknown,
        EnvState::Checking,
        EnvState::Missing,
        EnvState::Ready {
            build123d_version: String::new(),
        },
        EnvState::Failed("x".into()),
    ] {
        assert!(
            !app::sheets::dismiss_blocked(&state),
            "{state:?} must be dismissable"
        );
    }
}

/// The sheet's strings read through `fl!`; their keys resolve to the en
/// catalog values for the sheet's surfaces (PythonSetupSheet.swift:15-17,
/// 37, 63-80), verbatim. `select("en")` pins the shared loader so the host
/// locale cannot decide the outcome.
#[test]
fn sheet_strings_match_the_en_catalog() {
    app::localize::select("en");
    assert_eq!(
        app::localize::lookup("installazione-componenti"),
        "Installing Components"
    );
    assert_eq!(
        app::localize::lookup(
            "cad-studio-usa-un-ambiente-python-dedicato-con-build123d-matplotlib-e-numpy-per-generare-modelli-e-tavole-tecniche-l-installazione-scarica-circa-200-mb"
        ),
        "CAD Studio uses a dedicated Python environment with build123d, matplotlib and numpy to generate models and technical drawings. The installation downloads about 200 MB."
    );
    assert_eq!(app::localize::lookup("mostra-dettagli"), "Show Details");
    assert_eq!(app::localize::lookup("fine"), "Done");
    assert_eq!(app::localize::lookup("pi-tardi"), "Later");
    assert_eq!(app::localize::lookup("installa"), "Install");
    assert_eq!(
        app::localize::lookup("installazione-in-corso"),
        "Installing…"
    );
}

/// Element construction is pure (the viewer pane's established pattern):
/// the sheet renders one frame in every button shape, with the log
/// disclosure both closed and open.
#[test]
fn python_sheet_view_builds_in_every_button_shape() {
    for state in [
        EnvState::Unknown,
        EnvState::Checking,
        EnvState::Missing,
        EnvState::Installing,
        EnvState::Ready {
            build123d_version: "0.8.1".into(),
        },
        EnvState::Failed("The command “uv” failed. Check the log for details.".into()),
    ] {
        let mut sheet = app::sheets::PythonSetup::default();
        sheet.state.state = state;
        sheet.state.progress = 0.5;
        sheet.state.step = "Installing build123d, matplotlib and numpy…".into();
        sheet.state.log = "$ uv venv\nresolving…\n".into();
        let _ = app::sheets::view_python_setup(&sheet);
        sheet.shows_log = true;
        let _ = app::sheets::view_python_setup(&sheet);
    }
}
