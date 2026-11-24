//! Tests for playback-elsewhere detection, process-start restoration watcher, and COM ABI.
use super::*;

/// The PC's default devices: speakers for the game, a headset for voice.
fn speakers_and_headset() -> [Option<String>; 3] {
    [
        Some("{speakers}".into()),
        Some("{speakers}".into()),
        Some("{headset}".into()),
    ]
}

#[test]
fn a_game_that_opened_its_sound_before_the_route_is_reported() {
    let speakers = "{speakers}";
    let sink = "{steam}";
    let defaults = speakers_and_headset();
    let found = |pids: &[u32], sessions: &[(u32, String)]| {
        plays_elsewhere(pids, sessions, sink, &defaults, &[]).pids
    };
    let elsewhere = [(7, speakers.to_string())];
    assert_eq!(found(&[7], &elsewhere), [7]);
    let both = [
        (7, speakers.to_string()),
        (7, sink.to_string()),
        (8, sink.to_string()),
    ];
    assert_eq!(
        found(&[7, 8], &both),
        [7],
        "a second sound of the game still on the PC is reported beside one on the sink"
    );
    let on_sink = [(7, sink.to_string()), (8, sink.to_string())];
    assert!(found(&[7, 8], &on_sink).is_empty());
    assert!(
        found(&[9], &elsewhere).is_empty(),
        "another app is not the game"
    );
    assert!(found(&[7], &[]).is_empty(), "silent so far");
}

#[test]
fn voice_chat_on_its_own_communications_device_is_no_warning() {
    let sink = "{steam}";
    let voice = [(7, "{headset}".to_string()), (7, sink.to_string())];
    let found = plays_elsewhere(&[7], &voice, sink, &speakers_and_headset(), &[]);
    assert_eq!(found, OnThePc::default());
    let other = [(7, "{hdmi}".to_string())];
    assert_eq!(
        plays_elsewhere(&[7], &other, sink, &speakers_and_headset(), &[]),
        OnThePc::default(),
        "a device the PC doesn't play the game on is no one's"
    );
}

#[test]
fn the_games_own_device_counts_and_a_shared_communications_device_may_be_voice() {
    let sink = "{steam}";
    let own = [(7, "{hdmi}".to_string())];
    assert_eq!(
        plays_elsewhere(&[7], &own, sink, &speakers_and_headset(), &["{HDMI}", ""]),
        OnThePc {
            pids: vec![7],
            maybe_voice: false
        },
        "the game's own device from before the stream"
    );
    let shared = [
        Some("{speakers}".to_string()),
        Some("{speakers}".to_string()),
        Some("{speakers}".to_string()),
    ];
    let session = [(7, "{speakers}".to_string())];
    assert_eq!(
        plays_elsewhere(&[7], &session, sink, &shared, &[]),
        OnThePc {
            pids: vec![7],
            maybe_voice: true
        },
        "one device for both: the warning must allow for voice chat"
    );
    let on_default_sink = [Some(sink.to_string()), Some(sink.to_string()), None];
    assert_eq!(
        plays_elsewhere(
            &[7],
            &[(7, sink.to_string())],
            sink,
            &on_default_sink,
            &[sink]
        ),
        OnThePc::default(),
        "the sink is never the PC's"
    );
}

#[test]
fn a_per_app_device_names_its_endpoint() {
    assert_eq!(endpoint_id(HEADSET), "{headset}");
    assert_eq!(
        endpoint_id(&HEADSET.to_uppercase()),
        "{HEADSET}",
        "Windows' casing"
    );
    assert_eq!(endpoint_id(""), "");
}

/// Programs starting, as WMI tells them: each start also runs the program
/// in `fake`. `Err` ends the watch, as a broken subscription does.
/// A start of a process in `silent` leaves it without sound until the next
/// look that finds no start. Each look's timeout goes to `waited`.
struct Launches<'a> {
    fake: &'a Fake,
    events: std::collections::VecDeque<Result<Option<(u32, String)>>>,
    silent: &'a [u32],
    waited: Vec<Duration>,
}
impl<'a> Launches<'a> {
    fn new(fake: &'a Fake, events: Vec<Result<Option<(u32, String)>>>) -> Self {
        Self {
            fake,
            events: events.into(),
            silent: &[],
            waited: vec![],
        }
    }
}
impl Starts for Launches<'_> {
    fn next(&mut self, timeout: Duration) -> Result<Option<(u32, String)>> {
        self.waited.push(timeout);
        let event = self
            .events
            .pop_front()
            .unwrap_or_else(|| Err(anyhow::anyhow!("no more starts")))?;
        match &event {
            Some((pid, exe)) => {
                self.fake.running.borrow_mut().push((*pid, exe.clone()));
                if self.silent.contains(pid) {
                    self.fake.silent.borrow_mut().insert(*pid);
                }
            }
            None => self.fake.silent.borrow_mut().clear(),
        }
        Ok(event)
    }
}
/// A game routed to the sink that exited by itself: its restore waits.
fn quit_by_itself(fake: &Fake, directory: &Path) -> Result<Mutex<State>> {
    let mut state = State::default();
    started(fake, 7);
    fake.set(7, eConsole, HEADSET)?;
    route_with(fake, &mut state, directory, 7, GAME, SINK)?;
    exited(fake);
    assert!(release_with(fake, &mut state, directory, GAME)?);
    state.watching = true;
    Ok(Mutex::new(state))
}

#[test]
fn a_game_that_quit_by_itself_is_put_back_as_it_starts() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let fake = Fake::default();
    let state = quit_by_itself(&fake, directory.path())?;
    let mut starts = Launches::new(
        &fake,
        vec![
            Ok(Some((5, r"c:\windows\system32\notepad.exe".to_string()))),
            Ok(Some((9, GAME.to_string()))),
        ],
    );
    let listed = fake.listed.borrow().len();
    watch_starts(&fake, &state, directory.path(), &mut starts)?;
    assert_eq!(fake.device(GAME, eConsole), HEADSET);
    assert_eq!(fake.device(GAME, eMultimedia), "");
    assert_eq!(load(directory.path())?, Journal::default());
    assert!(!state.lock().unwrap().watching, "nothing waits: it ends");
    assert_eq!(
        fake.listed.borrow().len(),
        listed + 1,
        "only the first look lists processes: the start alone found the game"
    );
    Ok(())
}

#[test]
fn a_game_that_started_before_the_watch_is_found_by_its_first_look() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let fake = Fake::default();
    let state = quit_by_itself(&fake, directory.path())?;
    started(&fake, 9);
    let mut starts = Launches::new(&fake, vec![]);
    watch_starts(&fake, &state, directory.path(), &mut starts)?;
    assert_eq!(fake.device(GAME, eConsole), HEADSET);
    assert!(!state.lock().unwrap().watching);
    Ok(())
}

#[test]
fn a_broken_watch_of_starts_leaves_the_watcher_on_for_its_fallback() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let fake = Fake::default();
    let state = quit_by_itself(&fake, directory.path())?;
    let mut starts = Launches::new(&fake, vec![Ok(None), Err(anyhow::anyhow!("WMI stopped"))]);
    let error = watch_starts(&fake, &state, directory.path(), &mut starts).unwrap_err();
    assert_eq!(error.to_string(), "WMI stopped");
    assert!(state.lock().unwrap().watching, "the poll goes on from here");
    assert_eq!(load(directory.path())?.apps.len(), 1, "still waiting");
    assert_eq!(fake.device(GAME, eConsole), SINK);
    Ok(())
}

/// #59: Windows has no device for a process that hasn't opened its sound
/// (every game as it starts): the route changes and journals nothing, says
/// so, and works once the game opened it.
#[test]
fn a_game_without_sound_yet_is_routed_once_it_opened_it() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (fake, mut state) = (Fake::default(), State::default());
    started(&fake, 7);
    fake.silent.borrow_mut().insert(7);
    let error = route_with(&fake, &mut state, directory.path(), 7, GAME, SINK).unwrap_err();
    assert!(no_sound_yet(&error), "{error:#}");
    assert_eq!(load(directory.path())?, Journal::default());
    assert!(state.active.is_empty());
    fake.silent.borrow_mut().clear();
    assert_eq!(
        route_with(&fake, &mut state, directory.path(), 7, GAME, SINK)?,
        Routed::Sink
    );
    assert_eq!(fake.device(GAME, eConsole), SINK);
    Ok(())
}

/// A game put back as it starts has no sound yet: it is looked at again
/// within [`WATCH`], not at the next sweep, and nothing is reported.
#[test]
fn a_game_that_starts_without_sound_is_put_back_once_it_opened_it() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let fake = Fake::default();
    let state = quit_by_itself(&fake, directory.path())?;
    let mut starts = Launches::new(&fake, vec![Ok(Some((9, GAME.to_string()))), Ok(None)]);
    starts.silent = &[9];
    watch_starts(&fake, &state, directory.path(), &mut starts)?;
    assert_eq!(starts.waited[0], SWEEP);
    assert!(starts.waited[1] <= WATCH && starts.waited[1] > WATCH / 2);
    assert_eq!(fake.device(GAME, eConsole), HEADSET);
    assert_eq!(load(directory.path())?, Journal::default());
    assert!(!state.lock().unwrap().watching);
    Ok(())
}

/// Other programs starting meanwhile (a game's launch starts many) don't
/// put off that look: it comes [`WATCH`] after the game's start.
#[test]
fn other_starts_dont_put_off_the_look_at_a_game_without_sound() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let fake = Fake::default();
    let state = quit_by_itself(&fake, directory.path())?;
    let notepad = r"c:\windows\system32\notepad.exe".to_string();
    let mut starts = Launches::new(
        &fake,
        vec![
            Ok(Some((9, GAME.to_string()))),
            Ok(Some((5, notepad))),
            Ok(None),
        ],
    );
    starts.silent = &[9];
    // Each start comes a while after the last look.
    struct Slow<'a, 'b>(&'b mut Launches<'a>);
    impl Starts for Slow<'_, '_> {
        fn next(&mut self, timeout: Duration) -> Result<Option<(u32, String)>> {
            std::thread::sleep(Duration::from_millis(300));
            self.0.next(timeout)
        }
    }
    watch_starts(&fake, &state, directory.path(), &mut Slow(&mut starts))?;
    let waits = starts.waited;
    assert_eq!(waits[0], SWEEP);
    assert!(waits[1] <= WATCH);
    assert!(
        waits[2] + Duration::from_millis(250) < waits[1],
        "one deadline: {waits:?}"
    );
    assert_eq!(fake.device(GAME, eConsole), HEADSET);
    Ok(())
}

/// A game that never opens its sound is waited for, and said once after
/// [`SILENT_FOR`]; a look that isn't silent starts over.
#[test]
fn a_game_silent_for_long_is_said_once() {
    let mut state = State::default();
    let mut reported = Reported::default();
    assert!(waits_still(
        &mut state,
        Err(NoSoundYet.into()),
        &mut reported
    ));
    assert!(!reported.said_silent, "just started");
    reported.silent_since = Some(Instant::now() - SILENT_FOR);
    assert!(waits_still(
        &mut state,
        Err(NoSoundYet.into()),
        &mut reported
    ));
    assert!(reported.said_silent);
    assert!(waits_still(&mut state, Ok(true), &mut reported));
    assert_eq!(reported.silent_since, None);
    assert!(!reported.said_silent);
}

/// A program's helper that never opens its sound (a launcher stub, a crash
/// reporter) doesn't hold up its put-back: another of its processes that
/// opened its sound sets the program's device.
#[test]
fn a_helper_without_sound_doesnt_hold_up_the_put_back() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let fake = Fake::default();
    let state = quit_by_itself(&fake, directory.path())?;
    started(&fake, 8);
    started(&fake, 9);
    fake.silent.borrow_mut().insert(8);
    let state = state.into_inner().unwrap();
    assert!(!restore_with(&fake, &state, directory.path())?);
    assert_eq!(fake.device(GAME, eConsole), HEADSET);
    assert_eq!(load(directory.path())?, Journal::default());
    // Only silent ones: it waits for their sound.
    let directory = tempfile::tempdir()?;
    let fake = Fake::default();
    let state = quit_by_itself(&fake, directory.path())?
        .into_inner()
        .unwrap();
    started(&fake, 8);
    started(&fake, 9);
    fake.silent.borrow_mut().extend([8, 9]);
    let error = restore_with(&fake, &state, directory.path()).unwrap_err();
    assert!(no_sound_yet(&error), "{error:#}");
    assert_eq!(load(directory.path())?.apps.len(), 1);
    Ok(())
}

/// E_INVALIDARG is "no sound yet" only for a process Windows lists no
/// sound of; otherwise it is a failure that says so. Sessions are listed
/// only then.
#[test]
fn windows_refusing_a_process_without_sound_waits_for_it() {
    use windows::Win32::Foundation::{E_ACCESSDENIED, E_INVALIDARG, S_OK};
    let unasked = || -> Result<Vec<u32>> { panic!("sessions listed") };
    assert!(refusal(S_OK, 7, unasked).is_ok());
    let denied = refusal(E_ACCESSDENIED, 7, unasked).unwrap_err();
    assert!(!no_sound_yet(&denied));
    assert!(no_sound_yet(
        &refusal(E_INVALIDARG, 7, || Ok(vec![3, 9])).unwrap_err()
    ));
    let playing = refusal(E_INVALIDARG, 7, || Ok(vec![7])).unwrap_err();
    assert!(!no_sound_yet(&playing));
    assert!(format!("{playing:#}").contains("although the game opened its sound"));
    let unlisted = refusal(E_INVALIDARG, 7, || anyhow::bail!("no audio service")).unwrap_err();
    assert!(!no_sound_yet(&unlisted));
}

#[test]
fn policy_factory_abi_matches_windows() {
    let pointer = size_of::<usize>();
    assert_eq!(std::mem::offset_of!(FactoryVtbl, set), (6 + 19) * pointer);
    assert_eq!(std::mem::offset_of!(FactoryVtbl, get), (6 + 20) * pointer);
    assert_eq!(size_of::<FactoryVtbl>(), (6 + 22) * pointer);
}

/// The real factory on this PC, with no change: Windows has no device for
/// the test before it opens its sound (#59), and reads its own device back
/// after.
#[test]
#[ignore = "needs the Windows audio service"]
fn windows_reads_a_process_device() -> Result<()> {
    let _com = crate::os_helpers::ComGuard::new()?;
    let apps = Windows::new()?;
    let error = apps.get(std::process::id(), eMultimedia).unwrap_err();
    assert!(no_sound_yet(&error), "{error:#}");
    // A shared stream, opened and never started: silent.
    // SAFETY: COM is initialised by `_com`; every interface is owned here.
    let _client = unsafe {
        let enumerator: IMMDeviceEnumerator = windows::Win32::System::Com::CoCreateInstance(
            &MMDeviceEnumerator,
            None,
            windows::Win32::System::Com::CLSCTX_ALL,
        )?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let client: IAudioClient =
            device.Activate(windows::Win32::System::Com::CLSCTX_ALL, None)?;
        let format = client.GetMixFormat()?;
        client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 10_000_000, 0, format, None)?;
        client
    };
    let device = apps.get(std::process::id(), eMultimedia)?;
    assert!(
        device.is_empty() || device.starts_with(MMDEVAPI),
        "{device}"
    );
    Ok(())
}
