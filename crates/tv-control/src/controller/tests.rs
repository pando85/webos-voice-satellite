//! Controller tests. Fixtures are synthetic: field names from the design,
//! values such as `HDMI 1` and `App A` made up. No real device reply is
//! committed.

use super::*;
use crate::entities::{self as e, MediaFeatures, MediaPower, MediaState, StateValue as S};

const OWN_APP: &str = "app.own";

fn ms(t0: Instant, n: u64) -> Instant {
    t0 + Duration::from_millis(n)
}

// ----- fixtures -----------------------------------------------------------

fn power(state: &str) -> String {
    format!(r#"{{"returnValue":true,"state":"{state}"}}"#)
}

fn volume(v: u32, muted: bool, output: &str) -> String {
    format!(
        r#"{{"returnValue":true,"volumeStatus":{{"volume":{v},"maxVolume":60,"muteStatus":{muted},"soundOutput":"{output}","mode":"normal"}}}}"#
    )
}

fn fg(app: &str) -> String {
    format!(r#"{{"returnValue":true,"appId":"{app}","windowId":""}}"#)
}

const INPUTS: &str = r#"{"returnValue":true,"devices":[
    {"id":"HDMI_1","label":"HDMI 1","connected":true,"appId":"app.hdmi1","subList":[]},
    {"id":"HDMI_2","label":"HDMI 2","connected":false,"appId":"app.hdmi2"}]}"#;

const POINTS: &str = r#"{"returnValue":true,"launchPoints":[
    {"id":"app.a","title":"App A","icon":"/x/y.png","userData":{"t":"SECRET-TOKEN"}},
    {"id":"app.own","title":"Voice Satellite"},
    {"id":"app.hdmi1","title":"HDMI 1"},
    {"id":"app.b","title":"App B"}]}"#;

const PLAY: &str = r#"{"returnValue":true,"playState":"playing"}"#;

fn always_ready_reply(word: &str) -> String {
    format!(
        r#"{{"returnValue":true,"category":"general","settings":{{"lifeOnScreenMode":"{word}"}}}}"#
    )
}

fn activation_reply(on: bool) -> String {
    format!(
        r#"{{"returnValue":true,"settingStatus":{on},"activationState":{on},"thinQ":{{"settingStatus":{on},"activationState":{on}}},"amazon":{{"settingStatus":false,"activationState":false}},"dangbei":{{"settingStatus":false,"activationState":false}}}}"#
    )
}
const OK: &str = r#"{"returnValue":true}"#;

fn all_on() -> Config {
    Config {
        power_off: true,
        volume: true,
        mute: true,
        input: true,
        apps: true,
        play_state: true,
        own_app_ids: vec![OWN_APP.to_owned()],
        ..Config::default()
    }
}

/// The state of the fake TV the rig answers getters from.
#[derive(Clone)]
struct Tv {
    power: String,
    vol: u32,
    muted: bool,
    output: &'static str,
    fg: String,
    inputs: String,
    points: String,
    play: String,
    always_ready: String,
    activation: String,
}

impl Tv {
    fn new() -> Self {
        Self {
            power: "Active".to_owned(),
            vol: 12,
            muted: false,
            output: "external_arc",
            fg: "app.a".to_owned(),
            inputs: INPUTS.to_owned(),
            points: POINTS.to_owned(),
            play: PLAY.to_owned(),
            always_ready: always_ready_reply("off"),
            activation: activation_reply(false),
        }
    }

    fn answer(&self, topic: Topic) -> String {
        match topic {
            Topic::Power => power(&self.power),
            Topic::Volume => volume(self.vol, self.muted, self.output),
            Topic::Foreground => fg(&self.fg),
            Topic::Inputs => self.inputs.clone(),
            Topic::Apps => self.points.clone(),
            Topic::PlayState => self.play.clone(),
            Topic::AlwaysReady => self.always_ready.clone(),
            Topic::HandsFree => self.activation.clone(),
        }
    }
}

/// A controller with a fake TV behind it.
struct Rig {
    c: Controller,
    tv: Tv,
    t0: Instant,
    /// Every setter call made: (action, uri, payload).
    sets: Vec<(Action, &'static str, String)>,
    /// Every getter call made: (topic, uri, payload).
    reads: Vec<(Topic, &'static str, String)>,
    /// Every log line.
    logs: Vec<(Level, String)>,
    /// The last state per entity, as HA would see it.
    states: HashMap<&'static str, StateValue>,
    own_wakes: usize,
}

fn declare_all(c: &mut Controller) {
    for s in Setter::ALL {
        c.set_declared(s, Declared::Yes);
    }
}

impl Rig {
    /// Startup as the daemon does it: probe, answer every getter, finish.
    fn boot(cfg: Config, tv: Tv) -> Self {
        let mut c = Controller::new(cfg);
        declare_all(&mut c);
        let mut r = Self {
            c,
            tv,
            t0: Instant::now(),
            sets: Vec::new(),
            reads: Vec::new(),
            logs: Vec::new(),
            states: HashMap::new(),
            own_wakes: 0,
        };
        let fx = r.c.start();
        assert!(
            fx.iter().all(|f| matches!(
                f,
                Effect::Call {
                    purpose: Purpose::Read(_),
                    ..
                }
            )),
            "startup makes read-only calls only"
        );
        r.run(fx);
        assert!(r.c.probe_complete());
        let fx = r.c.finish_probe(r.t0);
        r.run(fx);
        let fx = r.c.set_env(PowerOffEnv {
            keep_listening: true,
            kill_switch: false,
            darkener: Ok(()),
        });
        r.run(fx);
        r
    }

    /// Takes effects: records states and logs, records setter calls, and
    /// answers getter calls from the fake TV (loop until quiet).
    fn run(&mut self, fx: Vec<Effect>) {
        let mut queue = fx;
        let mut guard = 0;
        while !queue.is_empty() {
            guard += 1;
            assert!(guard < 100, "effects do not settle");
            let mut next = Vec::new();
            for f in queue {
                match f {
                    Effect::State(u) => {
                        self.states.insert(u.id, u.value);
                    }
                    Effect::Log(l, m) => self.logs.push((l, m)),
                    Effect::OwnWake => self.own_wakes += 1,
                    Effect::Call {
                        purpose: Purpose::Read(t),
                        uri,
                        payload,
                    } => {
                        self.reads.push((t, uri, payload));
                        let text = self.tv.answer(t);
                        next.extend(self.c.on_reply(Purpose::Read(t), Ok(text), self.t0));
                    }
                    Effect::Call {
                        purpose: Purpose::Set(a),
                        uri,
                        payload,
                    } => {
                        self.sets.push((a, uri, payload));
                    }
                    Effect::Subscribe(_) | Effect::Unsubscribe(_) => {}
                }
            }
            queue = next;
        }
    }

    fn cmd(&mut self, cmd: Command, at: u64) -> &mut Self {
        let fx = self.c.on_command(cmd, ms(self.t0, at));
        self.run(fx);
        self
    }

    fn tick(&mut self, at: u64) -> &mut Self {
        let fx = self.c.tick(ms(self.t0, at));
        self.run(fx);
        self
    }

    fn power(&mut self, key: &str, processing: bool, at: u64) -> &mut Self {
        let fx = self.c.on_power(key, processing, ms(self.t0, at));
        self.run(fx);
        self
    }

    /// Answers the last setter call with `text` at `at`.
    fn set_reply(&mut self, text: &str, at: u64) -> &mut Self {
        let (a, _, _) = self.sets.last().cloned().expect("a setter call");
        let fx = self
            .c
            .on_reply(Purpose::Set(a), Ok(text.to_owned()), ms(self.t0, at));
        self.run(fx);
        self
    }

    fn state(&self, id: &str) -> Option<&StateValue> {
        self.states.get(id)
    }

    fn sw(&self, id: &str) -> Option<bool> {
        match self.state(id) {
            Some(S::Switch(v)) => *v,
            other => panic!("{id}: {other:?}"),
        }
    }

    fn num(&self, id: &str) -> Option<f32> {
        match self.state(id) {
            Some(S::Number(v)) => *v,
            other => panic!("{id}: {other:?}"),
        }
    }

    fn sel(&self, id: &str) -> Option<String> {
        match self.state(id) {
            Some(S::Select(v)) => v.clone(),
            other => panic!("{id}: {other:?}"),
        }
    }

    fn text(&self, id: &str) -> Option<String> {
        match self.state(id) {
            Some(S::Text(v)) => v.clone(),
            other => panic!("{id}: {other:?}"),
        }
    }

    fn media(&self) -> MediaState {
        match self.state(e::TV_MEDIA_PLAYER) {
            Some(S::Media(m)) => *m,
            other => panic!("tv_media_player: {other:?}"),
        }
    }

    fn set_names(&self) -> Vec<&'static str> {
        self.sets.iter().map(|(a, _, _)| a.name()).collect()
    }

    fn logged(&self, needle: &str) -> bool {
        self.logs.iter().any(|(_, m)| m.contains(needle))
    }
}

fn sw_cmd(id: &str, state: bool) -> Command {
    Command::Switch {
        id: id.to_owned(),
        state,
    }
}

fn btn(id: &str) -> Command {
    Command::Button { id: id.to_owned() }
}

fn num_cmd(id: &str, value: f32) -> Command {
    Command::Number {
        id: id.to_owned(),
        value,
    }
}

fn sel_cmd(id: &str, option: &str) -> Command {
    Command::Select {
        id: id.to_owned(),
        option: option.to_owned(),
    }
}

// ----- configuration and entities ------------------------------------------

#[test]
fn defaults_are_screen_power_on_and_the_checked_controls() {
    let c = Config::default();
    assert!(c.screen && c.power_on);
    assert!(
        c.volume && c.mute && c.input && c.apps,
        "on since EXP-010 Part A passed on c3-b"
    );
    assert!(
        !c.power_off && !c.play_state,
        "opt-in and extra polling stay off"
    );
    assert_eq!(
        (c.volume_limit, c.app_limit),
        (None, 30),
        "no cap by default"
    );
    assert_eq!(c.setters_blocked, None, "not gated by the firmware guard");
    assert!(c.media_player, "the TV media player is on (ADR 0007)");
    let ids: Vec<_> = Controller::new(c).entities().iter().map(|e| e.id).collect();
    assert_eq!(
        ids,
        [
            e::TV_MEDIA_PLAYER,
            e::TV_SCREEN,
            e::TV_TURN_ON,
            e::TV_VOLUME,
            e::TV_VOLUME_UP,
            e::TV_VOLUME_DOWN,
            e::TV_MUTE,
            e::TV_INPUT,
            e::TV_APP,
            e::TV_REFRESH_LISTS,
            e::TV_SOUND_OUTPUT,
            e::LG_ALWAYS_READY,
            e::LG_HANDS_FREE
        ]
    );
    // Everything off: only the screen switch, the turn-on button and the
    // media player that turns the TV on and off.
    let off = Config {
        volume: false,
        mute: false,
        input: false,
        apps: false,
        ..Config::default()
    };
    let c = Controller::new(off.clone());
    let ids: Vec<_> = c.entities().iter().map(|e| e.id).collect();
    assert_eq!(
        ids,
        [
            e::TV_MEDIA_PLAYER,
            e::TV_SCREEN,
            e::TV_TURN_ON,
            e::LG_ALWAYS_READY,
            e::LG_HANDS_FREE
        ]
    );
    assert_eq!(
        c.media_features(),
        MediaFeatures {
            turn_on: true,
            turn_off: true,
            ..MediaFeatures::default()
        }
    );
    // With nothing to offer, or switched off, it is not listed.
    for cfg in [
        Config {
            screen: false,
            power_on: false,
            ..off
        },
        Config {
            media_player: false,
            ..Config::default()
        },
    ] {
        let ids: Vec<_> = Controller::new(cfg)
            .entities()
            .iter()
            .map(|e| e.id)
            .collect();
        assert!(!ids.contains(&e::TV_MEDIA_PLAYER), "{ids:?}");
    }
}

#[test]
fn entity_table_follows_the_flags() {
    let r = Rig::boot(all_on(), Tv::new());
    let ents = r.c.entities();
    let ids: Vec<_> = ents.iter().map(|e| e.id).collect();
    assert_eq!(
        ids,
        [
            e::TV_MEDIA_PLAYER,
            e::TV_SCREEN,
            e::TV_TURN_ON,
            e::TV_POWER,
            e::TV_VOLUME,
            e::TV_VOLUME_UP,
            e::TV_VOLUME_DOWN,
            e::TV_MUTE,
            e::TV_INPUT,
            e::TV_APP,
            e::TV_REFRESH_LISTS,
            e::TV_SOUND_OUTPUT,
            e::TV_PLAY_STATE,
            e::LG_ALWAYS_READY,
            e::LG_HANDS_FREE
        ]
    );
    let by = |id| ents.iter().find(|e| e.id == id).unwrap();
    // The range comes from the TV, not a constant.
    assert_eq!(by(e::TV_VOLUME).kind, Kind::Number { max: 60.0 });
    // Inputs by label in the TV's order, the apps without the own app and the
    // input app, `Other` last.
    assert_eq!(
        by(e::TV_INPUT).kind,
        Kind::Select {
            options: vec!["HDMI 1".into(), "HDMI 2".into(), "Other".into()]
        }
    );
    assert_eq!(
        by(e::TV_APP).kind,
        Kind::Select {
            options: vec!["App A".into(), "App B".into(), "Other".into()]
        }
    );
    assert_eq!(by(e::TV_REFRESH_LISTS).category, Category::Config);
    assert_eq!(by(e::TV_SOUND_OUTPUT).category, Category::Diagnostic);
    assert_eq!(by(e::TV_PLAY_STATE).category, Category::Diagnostic);
    assert_eq!(by(e::TV_VOLUME).category, Category::None);
    assert_eq!(by(e::TV_MEDIA_PLAYER).category, Category::None);
    assert_eq!(by(e::TV_MEDIA_PLAYER).name, "TV");
    assert_eq!(
        by(e::TV_MEDIA_PLAYER).kind,
        Kind::MediaPlayer {
            features: MediaFeatures {
                volume_set: true,
                volume_step: true,
                mute: true,
                turn_on: true,
                turn_off: true
            }
        }
    );
    // power_off off: no tv_power; volume off: no number, no buttons, and
    // the media player has no volume features.
    let cfg = Config {
        power_off: false,
        volume: false,
        input: false,
        apps: false,
        play_state: false,
        lg_settings: false,
        ..all_on()
    };
    let c = Controller::new(cfg);
    let ids: Vec<_> = c.entities().iter().map(|e| e.id).collect();
    assert_eq!(
        ids,
        [
            e::TV_MEDIA_PLAYER,
            e::TV_SCREEN,
            e::TV_TURN_ON,
            e::TV_MUTE,
            e::TV_SOUND_OUTPUT
        ]
    );
    let f = c.media_features();
    assert!(!f.volume_set && !f.volume_step && f.mute && f.turn_on && f.turn_off);
}

#[test]
fn startup_makes_no_write_and_seeds_the_states() {
    let r = Rig::boot(all_on(), Tv::new());
    assert!(r.sets.is_empty(), "never a probe write");
    assert_eq!(r.sw(e::TV_SCREEN), Some(true));
    assert_eq!(r.num(e::TV_VOLUME), Some(12.0));
    assert_eq!(r.sw(e::TV_MUTE), Some(false));
    assert_eq!(r.sel(e::TV_APP).as_deref(), Some("App A"));
    assert_eq!(r.sel(e::TV_INPUT).as_deref(), Some("Other"));
    assert_eq!(r.text(e::TV_SOUND_OUTPUT).as_deref(), Some("external_arc"));
    assert_eq!(r.text(e::TV_PLAY_STATE).as_deref(), Some("playing"));
    // tv_power: the conditions hold (keep listening, no kill, darkener ok).
    assert_eq!(r.sw(e::TV_POWER), Some(true));
}

#[test]
fn the_foreground_input_app_selects_the_input() {
    let mut tv = Tv::new();
    tv.fg = "app.hdmi2".to_owned();
    let r = Rig::boot(all_on(), tv);
    assert_eq!(r.sel(e::TV_INPUT).as_deref(), Some("HDMI 2"));
    assert_eq!(
        r.sel(e::TV_APP).as_deref(),
        Some("Other"),
        "an input app is not in the app list"
    );
}

// ----- availability per power state (design 3.4) -------------------------

#[test]
fn availability_per_power_state() {
    let mut r = Rig::boot(all_on(), Tv::new());
    // Active: everything live.
    assert_eq!(r.sw(e::TV_SCREEN), Some(true));
    // Screen Off: screen off, volume still live, play state missing.
    r.power("ScreenOff", false, 1000);
    assert_eq!(r.sw(e::TV_SCREEN), Some(false));
    assert_eq!(r.sw(e::TV_POWER), Some(false));
    assert_eq!(r.num(e::TV_VOLUME), Some(12.0));
    assert_eq!(r.sel(e::TV_APP).as_deref(), Some("App A"));
    assert_eq!(r.text(e::TV_PLAY_STATE), None);
    // A transition keeps the last settled state.
    r.power("ScreenOff", true, 2000);
    assert_eq!(r.sw(e::TV_SCREEN), Some(false));
    assert_eq!(r.num(e::TV_VOLUME), Some(12.0));
    // Active Standby: screen, volume, mute, input and app are missing; the
    // power switch reads off; the sound output keeps its last value.
    r.power("ActiveStandby", false, 3000);
    assert_eq!(r.sw(e::TV_SCREEN), None);
    assert_eq!(r.sw(e::TV_POWER), Some(false));
    assert_eq!(r.num(e::TV_VOLUME), None);
    assert_eq!(r.sw(e::TV_MUTE), None);
    assert_eq!(r.sel(e::TV_INPUT), None);
    assert_eq!(r.sel(e::TV_APP), None);
    assert_eq!(r.text(e::TV_SOUND_OUTPUT).as_deref(), Some("external_arc"));
    assert_eq!(r.text(e::TV_PLAY_STATE), None);
    // Screen Saver counts as on.
    r.power("ScreenSaver", false, 4000);
    assert_eq!(r.sw(e::TV_SCREEN), Some(true));
    assert_eq!(r.num(e::TV_VOLUME), Some(12.0));
    assert_eq!(r.text(e::TV_PLAY_STATE).as_deref(), Some("playing"));
}

#[test]
fn commands_are_refused_where_the_state_is_missing() {
    let mut r = Rig::boot(all_on(), Tv::new());
    r.power("ActiveStandby", false, 1000);
    for c in [
        sw_cmd(e::TV_SCREEN, false),
        sw_cmd(e::TV_MUTE, true),
        btn(e::TV_VOLUME_UP),
        btn(e::TV_VOLUME_DOWN),
        sel_cmd(e::TV_INPUT, "HDMI 2"),
        sel_cmd(e::TV_APP, "App B"),
        sw_cmd(e::TV_POWER, false),
    ] {
        r.cmd(c, 5000);
    }
    r.cmd(num_cmd(e::TV_VOLUME, 20.0), 5000).tick(6000);
    assert!(
        r.sets.is_empty(),
        "nothing is sent in standby: {:?}",
        r.set_names()
    );
    // A transition refuses too.
    r.power("Active", false, 7000)
        .power("ScreenOff", true, 7100);
    r.cmd(sw_cmd(e::TV_SCREEN, true), 8000)
        .cmd(btn(e::TV_VOLUME_UP), 8000);
    assert!(r.sets.is_empty());
    assert!(r.logged("refused"));
}

// ----- screen, turn on ------------------------------------------------------

#[test]
fn screen_off_and_on_use_the_verified_calls() {
    let mut r = Rig::boot(all_on(), Tv::new());
    r.cmd(sw_cmd(e::TV_SCREEN, false), 1000);
    assert_eq!(r.sets.len(), 1);
    assert_eq!(r.sets[0].1, setters::URI_SCREEN_OFF);
    assert_eq!(r.sets[0].2, "{}");
    r.set_reply(r#"{"returnValue":true,"state":"Screen Off"}"#, 1080);
    // The state follows the power subscription, not the command.
    assert_eq!(r.sw(e::TV_SCREEN), Some(true));
    r.power("ScreenOff", false, 1100);
    assert_eq!(r.sw(e::TV_SCREEN), Some(false));
    // On from Screen Off: turnOnScreen, and the darkener is told it is ours.
    r.cmd(sw_cmd(e::TV_SCREEN, true), 5000);
    assert_eq!(r.sets[1].1, setters::URI_SCREEN_ON);
    assert_eq!(r.own_wakes, 1);
    r.set_reply(r#"{"returnValue":true,"state":"Active"}"#, 5100)
        .power("Active", false, 5100);
    assert_eq!(r.sw(e::TV_SCREEN), Some(true));
    // Already on / already off is a no-op.
    let n = r.sets.len();
    r.cmd(sw_cmd(e::TV_SCREEN, true), 9000);
    assert_eq!(r.sets.len(), n);
}

#[test]
fn turn_on_by_power_state() {
    let mut r = Rig::boot(all_on(), Tv::new());
    // Active: a no-op.
    r.cmd(btn(e::TV_TURN_ON), 1000);
    assert!(r.sets.is_empty());
    // Screen Off: turnOnScreen.
    r.power("ScreenOff", false, 2000)
        .cmd(btn(e::TV_TURN_ON), 3000);
    assert_eq!(r.set_names(), ["turnOnScreen"]);
    r.set_reply(OK, 3100);
    // Active Standby: powerOn after the 1.5 s key block of the settle, with
    // the payload verified on c3-b; a Keyblock answer is retried once.
    r.power("ActiveStandby", false, 10_000)
        .cmd(btn(e::TV_TURN_ON), 10_200);
    r.tick(10_400);
    assert_eq!(r.sets.len(), 1, "waits for the key block");
    r.tick(11_500);
    assert_eq!(r.sets.len(), 2);
    assert_eq!(r.sets[1].1, setters::URI_POWER_ON);
    assert_eq!(r.sets[1].2, r#"{"reason":"remoteKey"}"#);
    r.set_reply(
        r#"{"returnValue":false,"errorCode":-1,"errorText":"Keyblock1.5second"}"#,
        11_600,
    );
    r.tick(12_000);
    assert_eq!(r.sets.len(), 2, "the retry waits 1.6 s");
    r.tick(13_300);
    assert_eq!(r.sets.len(), 3);
    assert_eq!(r.set_names(), ["turnOnScreen", "powerOn", "powerOn"]);
    r.set_reply(
        r#"{"returnValue":false,"errorCode":-1,"errorText":"Keyblock1.5second"}"#,
        13_400,
    );
    r.tick(20_000);
    assert_eq!(r.sets.len(), 3, "only one retry");
    assert_eq!(r.own_wakes, 3);
    // A transition refuses.
    r.power("ActiveStandby", true, 30_000)
        .cmd(btn(e::TV_TURN_ON), 30_100)
        .tick(40_000);
    assert_eq!(r.sets.len(), 3);
}

#[test]
fn a_failed_screen_call_is_logged_and_does_not_disable_the_switch() {
    let mut r = Rig::boot(all_on(), Tv::new());
    r.cmd(sw_cmd(e::TV_SCREEN, false), 1000);
    r.set_reply(
        r#"{"returnValue":false,"errorCode":-1,"errorText":"The current substate must be 'active'"}"#,
        1050,
    );
    assert!(r.logged("turnOffScreen failed"));
    assert_eq!(r.sw(e::TV_SCREEN), Some(true), "the real state is shown");
    r.cmd(sw_cmd(e::TV_SCREEN, false), 5000);
    assert_eq!(
        r.sets.len(),
        2,
        "tvpowerd's transient refusal does not switch the control off"
    );
}

// ----- power-off (design 4.2) -------------------------------------------------

fn env(keep: bool, kill: bool, darkener: Result<(), &str>) -> PowerOffEnv {
    PowerOffEnv {
        keep_listening: keep,
        kill_switch: kill,
        darkener: darkener.map_err(str::to_owned),
    }
}

#[test]
fn power_off_sends_the_key_when_every_condition_holds() {
    let mut r = Rig::boot(all_on(), Tv::new());
    assert_eq!(r.c.power_off_check(), Ok(PowerOffRoute::KeepListening));
    r.cmd(sw_cmd(e::TV_POWER, false), 1000);
    assert_eq!(r.sets.len(), 1);
    assert_eq!(r.sets[0].1, setters::URI_POWER_KEY);
    assert_eq!(r.sets[0].2, "{}");
    r.set_reply(OK, 1050);
    // No read-back and no state guess: the power subscription shows the rest.
    assert_eq!(r.sw(e::TV_POWER), Some(true));
    r.power("ActiveStandby", true, 1100)
        .power("ActiveStandby", false, 4000);
    assert_eq!(r.sw(e::TV_POWER), Some(false));
    // Screen Saver works like Active.
    let mut r = Rig::boot(all_on(), Tv::new());
    r.power("ScreenSaver", false, 100);
    assert_eq!(r.c.power_off_check(), Ok(PowerOffRoute::KeepListening));
}

#[test]
fn power_off_precondition_matrix() {
    // Each precondition on its own, every other one holding.
    let cases: [(&str, Config, PowerOffEnv, &str, PowerOffRefusal); 9] = [
        (
            "Turn off normally with Always Ready off (the fake TV's value)",
            all_on(),
            env(false, false, Err("not keep listening")),
            "Active",
            PowerOffRefusal::AlwaysReadyOff,
        ),
        (
            "firmware guard",
            Config {
                setters_blocked: Some("firmware".into()),
                ..all_on()
            },
            env(true, false, Ok(())),
            "Active",
            PowerOffRefusal::SettersBlocked("firmware".into()),
        ),
        (
            "Turn off normally with the LG settings sensors off (Always Ready unknown)",
            Config {
                lg_settings: false,
                ..all_on()
            },
            env(false, false, Err("not keep listening")),
            "Active",
            PowerOffRefusal::AlwaysReadyUnknown,
        ),
        (
            "kill switch",
            all_on(),
            env(true, true, Ok(())),
            "Active",
            PowerOffRefusal::KillSwitch,
        ),
        (
            "darkener cooldown",
            all_on(),
            env(true, false, Err("cooldown")),
            "Active",
            PowerOffRefusal::Darkener("cooldown".into()),
        ),
        (
            "darkener hourly budget",
            all_on(),
            env(true, false, Err("10 actions in the last hour")),
            "Active",
            PowerOffRefusal::Darkener("10 actions in the last hour".into()),
        ),
        (
            "Screen Off (the key would be a second press)",
            all_on(),
            env(true, false, Ok(())),
            "ScreenOff",
            PowerOffRefusal::WrongState,
        ),
        (
            "Active Standby",
            all_on(),
            env(true, false, Ok(())),
            "ActiveStandby",
            PowerOffRefusal::WrongState,
        ),
        (
            "Suspend-like unknown state",
            all_on(),
            env(true, false, Ok(())),
            "SomethingElse",
            PowerOffRefusal::WrongState,
        ),
    ];
    for (name, cfg, e, power_state, want) in cases {
        let mut r = Rig::boot(cfg, Tv::new());
        r.c.set_env(e);
        r.power(power_state, false, 100);
        assert_eq!(r.c.power_off_check(), Err(want.clone()), "{name}");
        r.cmd(sw_cmd(e::TV_POWER, false), 1000).tick(2000);
        assert!(
            !r.sets
                .iter()
                .any(|(_, uri, _)| *uri == setters::URI_POWER_KEY),
            "{name}: no key may be sent"
        );
    }
    // A transition in progress.
    let mut r = Rig::boot(all_on(), Tv::new());
    r.power("Active", true, 100);
    assert_eq!(r.c.power_off_check(), Err(PowerOffRefusal::Transition));
    r.cmd(sw_cmd(e::TV_POWER, false), 1000);
    assert!(r.sets.is_empty());
    // No power state yet.
    let mut c = Controller::new(all_on());
    c.set_env(env(true, false, Ok(())));
    assert_eq!(c.power_off_check(), Err(PowerOffRefusal::PowerStateUnknown));
    // The daemon has not said anything yet: nothing is allowed.
    let mut c = Controller::new(all_on());
    c.on_power("Active", false, Instant::now());
    assert_eq!(c.power_off_check(), Err(PowerOffRefusal::NotReady));
    // The daemon's first view, even if it equals the default, counts.
    c.set_env(PowerOffEnv::default());
    assert_eq!(
        c.power_off_check(),
        Err(PowerOffRefusal::AlwaysReadyUnknown)
    );
}

/// A fake TV with Always Ready on (`alwaysReady` or `allEnabled`).
fn tv_always_ready(word: &str) -> Tv {
    Tv {
        always_ready: always_ready_reply(word),
        ..Tv::new()
    }
}

/// A rig on "Turn off normally" (the darkener would not act in that mode).
fn boot_normal(cfg: Config, tv: Tv) -> Rig {
    let mut r = Rig::boot(cfg, tv);
    let fx = r.c.set_env(env(false, false, Err("not keep listening")));
    r.run(fx);
    r
}

#[test]
fn power_off_in_normal_mode_needs_always_ready_on() {
    for word in ["alwaysReady", "allEnabled"] {
        let r = boot_normal(all_on(), tv_always_ready(word));
        assert_eq!(r.c.power_off_check(), Ok(PowerOffRoute::Standby), "{word}");
    }
    for (word, want) in [
        ("off", PowerOffRefusal::AlwaysReadyOff),
        ("none", PowerOffRefusal::AlwaysReadyOff),
        ("lifeOnScreen", PowerOffRefusal::AlwaysReadyUnknown),
    ] {
        let r = boot_normal(all_on(), tv_always_ready(word));
        assert_eq!(r.c.power_off_check(), Err(want.clone()), "{word}");
        assert!(want.screen_off_instead());
    }
    // A failed Always Ready read: unknown.
    let mut tv = Tv::new();
    tv.always_ready = r#"{"returnValue":false,"errorText":"x"}"#.to_owned();
    let r = boot_normal(all_on(), tv);
    assert_eq!(
        r.c.power_off_check(),
        Err(PowerOffRefusal::AlwaysReadyUnknown)
    );
    // Always Ready does not matter in the keep-listening mode (the darkener
    // wakes the TV before LG's suspend, P3 of the voice and power screen).
    let r = Rig::boot(all_on(), Tv::new());
    assert_eq!(r.c.power_off_check(), Ok(PowerOffRoute::KeepListening));
    // Other refusals are never a reason for the screen-off fallback.
    for why in [
        PowerOffRefusal::NotReady,
        PowerOffRefusal::SettersBlocked("x".into()),
        PowerOffRefusal::PowerStateUnknown,
        PowerOffRefusal::Transition,
        PowerOffRefusal::WrongState,
        PowerOffRefusal::KillSwitch,
        PowerOffRefusal::Darkener("cooldown".into()),
    ] {
        assert!(!why.screen_off_instead(), "{why}");
    }
}

#[test]
fn the_power_switch_in_normal_mode_with_always_ready_on() {
    let mut r = boot_normal(all_on(), tv_always_ready("alwaysReady"));
    assert_eq!(r.sw(e::TV_POWER), Some(true));
    r.cmd(sw_cmd(e::TV_POWER, false), 1000);
    assert_eq!(r.set_names(), ["power key"]);
    assert!(r.logged("Active Standby"));
    r.set_reply(OK, 1050)
        .power("ActiveStandby", true, 1100)
        .power("ActiveStandby", false, 4000);
    assert_eq!(r.sw(e::TV_POWER), Some(false));
    // On again: powerOn after the key block.
    r.cmd(sw_cmd(e::TV_POWER, true), 4100).tick(5600);
    assert_eq!(r.set_names(), ["power key", "powerOn"]);
    // Always Ready off: no fallback for the switch (that is tv_screen).
    let mut r = boot_normal(all_on(), Tv::new());
    assert_eq!(r.sw(e::TV_POWER), None);
    r.cmd(sw_cmd(e::TV_POWER, false), 1000);
    assert!(r.sets.is_empty());
    assert!(r.logged("Always Ready is off"));
}

#[test]
fn the_power_switch_reads_unknown_and_refuses_off_without_the_conditions() {
    // B6: the select on "Turn off normally" and Always Ready off.
    let mut r = Rig::boot(all_on(), Tv::new());
    let fx = r.c.set_env(env(false, false, Err("not keep listening")));
    r.run(fx);
    assert_eq!(r.sw(e::TV_POWER), None);
    r.cmd(sw_cmd(e::TV_POWER, false), 1000)
        .cmd(sw_cmd(e::TV_POWER, true), 1100);
    assert!(r.sets.is_empty());
    // The kill switch (B9) refuses the off key. On is a turn on, as the
    // media player's and tv_turn_on: not gated by the power-off conditions
    // (2026-11-09 review).
    let mut r = Rig::boot(all_on(), Tv::new());
    let fx = r.c.set_env(env(true, true, Ok(())));
    r.run(fx);
    assert_eq!(r.sw(e::TV_POWER), None);
    r.cmd(sw_cmd(e::TV_POWER, false), 1000);
    assert!(r.sets.is_empty());
    r.power("ScreenOff", false, 2000)
        .cmd(sw_cmd(e::TV_POWER, true), 3000);
    assert_eq!(r.set_names(), ["turnOnScreen"], "the way back is not gated");
    // The conditions come back: the state is shown again.
    let fx = r.c.set_env(env(true, false, Ok(())));
    r.run(fx);
    assert_eq!(r.sw(e::TV_POWER), Some(false));
}

#[test]
fn power_on_from_the_power_switch() {
    let mut r = Rig::boot(all_on(), Tv::new());
    r.power("ScreenOff", false, 1000)
        .cmd(sw_cmd(e::TV_POWER, true), 2000);
    assert_eq!(r.set_names(), ["turnOnScreen"]);
    assert_eq!(r.own_wakes, 1);
    let mut r = Rig::boot(all_on(), Tv::new());
    r.power("ActiveStandby", false, 1000)
        .cmd(sw_cmd(e::TV_POWER, true), 1100)
        .tick(2600);
    assert_eq!(r.set_names(), ["powerOn"]);
    // On while already on, off while already off: nothing to do.
    let mut r = Rig::boot(all_on(), Tv::new());
    r.cmd(sw_cmd(e::TV_POWER, true), 1000);
    r.power("ScreenOff", false, 2000)
        .cmd(sw_cmd(e::TV_POWER, false), 3000);
    assert!(r.sets.is_empty());
}

#[test]
fn power_off_is_not_counted_in_the_global_budget_but_one_at_a_time() {
    let mut r = Rig::boot(all_on(), Tv::new());
    r.cmd(sw_cmd(e::TV_POWER, false), 1000)
        .cmd(sw_cmd(e::TV_POWER, false), 1100);
    assert_eq!(r.sets.len(), 1, "one call in flight");
}

// ----- volume, mute -----------------------------------------------------------

#[test]
fn volume_set_is_coalesced_clamped_and_read_back() {
    let mut r = Rig::boot(all_on(), Tv::new());
    // Three slider moves within 250 ms: the latest wins, one call.
    r.cmd(num_cmd(e::TV_VOLUME, 15.0), 1000)
        .cmd(num_cmd(e::TV_VOLUME, 17.0), 1100)
        .cmd(num_cmd(e::TV_VOLUME, 18.0), 1200)
        .tick(1300);
    assert!(r.sets.is_empty(), "still coalescing");
    r.tick(1460);
    assert_eq!(r.sets.len(), 1);
    assert_eq!(
        (r.sets[0].1, r.sets[0].2.as_str()),
        (setters::URI_SET_VOLUME, r#"{"volume":18}"#)
    );
    // The reply, then the read-back at 0.5 s shows the TV's value.
    r.set_reply(r#"{"returnValue":true,"volume":18,"soundOutput":""}"#, 1500);
    r.tv.vol = 18;
    r.tick(1900);
    assert_eq!(
        r.num(e::TV_VOLUME),
        Some(12.0),
        "the state comes from the TV, not the command"
    );
    r.tick(2010);
    assert_eq!(r.num(e::TV_VOLUME), Some(18.0));
    assert!(r.logged("read back matches"));
    assert!(r.c.check.is_none());
}

#[test]
fn volume_without_a_cap_goes_up_to_the_tv_maximum() {
    // The default: no cap below maxVolume (c3-a's owner uses 89 on a TV
    // whose maximum is higher than c3-b's 60).
    let mut tv = Tv::new();
    tv.vol = 55;
    let mut r = Rig::boot(all_on(), tv);
    assert_eq!(
        r.c.entities()
            .iter()
            .find(|e| e.id == e::TV_VOLUME)
            .map(|e| e.kind.clone()),
        Some(Kind::Number { max: 60.0 })
    );
    r.cmd(num_cmd(e::TV_VOLUME, 58.0), 1000).tick(1300);
    assert_eq!(r.sets[0].2, r#"{"volume":58}"#);
    assert!(!r.logged("ceiling"));
    // Volume up works above the old default of 40.
    let mut tv = Tv::new();
    tv.vol = 55;
    let mut r = Rig::boot(all_on(), tv);
    r.cmd(btn(e::TV_VOLUME_UP), 1000);
    assert_eq!(r.sets[0].1, setters::URI_VOLUME_UP);
}

#[test]
fn a_cap_clamps_sets_limits_the_slider_and_still_lets_the_volume_come_down() {
    let cap = || Config {
        volume_limit: Some(40),
        ..all_on()
    };
    let r = Rig::boot(cap(), Tv::new());
    assert_eq!(
        r.c.entities()
            .iter()
            .find(|e| e.id == e::TV_VOLUME)
            .map(|e| e.kind.clone()),
        Some(Kind::Number { max: 40.0 }),
        "the slider ends at the cap"
    );
    let mut r = Rig::boot(cap(), Tv::new());
    r.cmd(num_cmd(e::TV_VOLUME, 55.0), 1000).tick(1300);
    assert_eq!(r.sets[0].2, r#"{"volume":40}"#, "volume_limit 40");
    assert!(r.logged("ceiling"));
    // The TV is above the cap (set at the TV): any value up to the cap and
    // volume down still work; volume up is refused.
    let mut tv = Tv::new();
    tv.vol = 55;
    let mut r = Rig::boot(cap(), tv);
    r.cmd(btn(e::TV_VOLUME_UP), 1000);
    assert!(r.sets.is_empty());
    assert!(r.logged("at the volume limit"));
    r.cmd(num_cmd(e::TV_VOLUME, 30.0), 3000).tick(3300);
    assert_eq!(r.sets[0].2, r#"{"volume":30}"#);
    let mut tv = Tv::new();
    tv.vol = 55;
    let mut r = Rig::boot(cap(), tv);
    r.cmd(btn(e::TV_VOLUME_DOWN), 1000);
    assert_eq!(r.sets[0].1, setters::URI_VOLUME_DOWN);
}

#[test]
fn volume_above_the_limit_is_clamped_and_visible() {
    let cfg = Config {
        volume_limit: Some(40),
        ..all_on()
    };
    let mut r = Rig::boot(cfg, Tv::new());
    r.cmd(num_cmd(e::TV_VOLUME, 55.0), 1000).tick(1300);
    assert_eq!(r.sets[0].2, r#"{"volume":40}"#, "volume_limit 40");
    assert!(r.logged("ceiling"));
    // Above the TV's own maximum: also capped (by the lower of the two).
    let cfg = Config {
        volume_limit: Some(100),
        ..all_on()
    };
    let mut r = Rig::boot(cfg, Tv::new());
    r.cmd(num_cmd(e::TV_VOLUME, 99.0), 1000).tick(1300);
    assert_eq!(r.sets[0].2, r#"{"volume":60}"#, "maxVolume 60");
    // Not a volume at all.
    let mut r = Rig::boot(all_on(), Tv::new());
    r.cmd(num_cmd(e::TV_VOLUME, f32::NAN), 1000)
        .cmd(num_cmd(e::TV_VOLUME, -3.0), 1000)
        .tick(2000);
    assert!(r.sets.is_empty());
    // The same value as now: nothing to do.
    r.cmd(num_cmd(e::TV_VOLUME, 12.0), 3000).tick(3500);
    assert!(r.sets.is_empty());
}

#[test]
fn volume_steps_stop_at_the_limit_and_are_rate_limited() {
    let mut tv = Tv::new();
    tv.vol = 39;
    let cfg = Config {
        volume_limit: Some(40),
        ..all_on()
    };
    let mut r = Rig::boot(cfg, tv);
    r.cmd(btn(e::TV_VOLUME_UP), 1000);
    assert_eq!(
        (r.sets[0].1, r.sets[0].2.as_str()),
        (setters::URI_VOLUME_UP, "{}")
    );
    r.set_reply(r#"{"returnValue":true,"volume":40,"soundOutput":""}"#, 1030);
    r.tv.vol = 40;
    r.tick(1600).tick(1700);
    assert_eq!(r.num(e::TV_VOLUME), Some(40.0));
    // At the limit: up is refused without a call, down works.
    r.cmd(btn(e::TV_VOLUME_UP), 3000);
    assert_eq!(r.sets.len(), 1);
    assert!(r.logged("at the volume limit"));
    r.cmd(btn(e::TV_VOLUME_DOWN), 3100);
    assert_eq!(r.sets[1].1, setters::URI_VOLUME_DOWN);
    // Volume 0: down is a no-op.
    let mut tv = Tv::new();
    tv.vol = 0;
    let mut r = Rig::boot(all_on(), tv);
    r.cmd(btn(e::TV_VOLUME_DOWN), 1000);
    assert!(r.sets.is_empty());
    // Four steps in one second, the fifth is refused.
    let mut r = Rig::boot(all_on(), Tv::new());
    for n in 0..5 {
        r.cmd(btn(e::TV_VOLUME_UP), 1000 + n * 50);
        if r.sets.len() as u64 > n {
            r.set_reply(r#"{"returnValue":true,"volume":13}"#, 1000 + n * 50 + 10);
            r.c.check = None;
        }
    }
    assert_eq!(r.sets.len(), 4);
    assert!(r.logged("limited to 4 per second"));
}

#[test]
fn mute_uses_the_verified_setter_and_reads_back() {
    let mut r = Rig::boot(all_on(), Tv::new());
    r.cmd(sw_cmd(e::TV_MUTE, true), 1000);
    assert_eq!(
        (r.sets[0].1, r.sets[0].2.as_str()),
        (setters::URI_SET_MUTE, r#"{"mute":true}"#)
    );
    r.set_reply(
        r#"{"returnValue":true,"muteStatus":true,"soundOutput":"external_arc"}"#,
        1030,
    );
    r.tv.muted = true;
    r.tick(1600);
    assert_eq!(r.sw(e::TV_MUTE), Some(true));
    // Already muted: nothing to do.
    r.cmd(sw_cmd(e::TV_MUTE, true), 5000);
    assert_eq!(r.sets.len(), 1);
}

#[test]
fn a_read_back_mismatch_disables_the_control_and_never_retries_with_other_parameters() {
    let mut r = Rig::boot(all_on(), Tv::new());
    r.cmd(num_cmd(e::TV_VOLUME, 20.0), 1000).tick(1300);
    r.set_reply(OK, 1330);
    // The TV keeps 12 (as an ARC soundbar might): two reads, then disabled.
    r.tick(1900);
    assert_eq!(
        r.c.check.as_ref().map(|c| c.reads_left),
        Some(0),
        "one more read after the first mismatch"
    );
    r.tick(4500);
    assert!(r.logged("read-back mismatch"));
    assert_eq!(r.num(e::TV_VOLUME), None, "no state: it cannot be trusted");
    assert_eq!(r.sets.len(), 1, "no second write");
    r.cmd(num_cmd(e::TV_VOLUME, 20.0), 10_000)
        .cmd(btn(e::TV_VOLUME_UP), 10_100)
        .tick(11_000);
    assert_eq!(r.sets.len(), 1, "disabled: commands refused");
    // Mute is a separate control and still works.
    r.cmd(sw_cmd(e::TV_MUTE, true), 12_000);
    assert_eq!(r.sets.len(), 2);
}

#[test]
fn a_setter_that_fails_disables_its_control_only() {
    let mut r = Rig::boot(all_on(), Tv::new());
    r.cmd(sel_cmd(e::TV_APP, "App B"), 1000);
    r.set_reply(
        r#"{"returnValue":false,"errorCode":-101,"errorText":"not exist"}"#,
        1100,
    );
    assert!(r.logged("launch failed: luna error -101: not exist"));
    assert_eq!(r.sel(e::TV_APP), None);
    r.cmd(sel_cmd(e::TV_APP, "App A"), 9000);
    assert_eq!(r.sets.len(), 1);
    // The input select is separate.
    r.cmd(sel_cmd(e::TV_INPUT, "HDMI 2"), 9000);
    assert_eq!(r.sets.len(), 2);
}

#[test]
fn a_transport_failure_of_a_setter_disables_an_untested_control_with_one_warning() {
    let mut r = Rig::boot(all_on(), Tv::new());
    r.cmd(sw_cmd(e::TV_MUTE, true), 1000);
    let fx = r.c.on_reply(
        Purpose::Set(Action::Mute(true)),
        Err("no answer within 5 s".to_owned()),
        ms(r.t0, 6000),
    );
    r.run(fx);
    assert_eq!(r.sw(e::TV_MUTE), None);
    let warns = r
        .logs
        .iter()
        .filter(|(l, m)| *l == Level::Warn && m.contains("mute"))
        .count();
    assert!(warns >= 1);
}

// ----- input, app --------------------------------------------------------------

#[test]
fn input_and_app_commands_map_labels_to_ids() {
    let mut r = Rig::boot(all_on(), Tv::new());
    r.cmd(sel_cmd(e::TV_INPUT, "HDMI 2"), 1000);
    assert_eq!(
        (r.sets[0].1, r.sets[0].2.as_str()),
        (setters::URI_SWITCH_INPUT, r#"{"inputId":"HDMI_2"}"#)
    );
    r.set_reply(OK, 1440);
    r.tv.fg = "app.hdmi2".to_owned();
    r.tick(2000).tick(2100);
    assert_eq!(r.sel(e::TV_INPUT).as_deref(), Some("HDMI 2"));
    assert!(r.logged("read back matches"));
    // An app: launch with the app id; the read-back may need the second read.
    r.cmd(sel_cmd(e::TV_APP, "App B"), 5000);
    assert_eq!(
        (r.sets[1].1, r.sets[1].2.as_str()),
        (setters::URI_LAUNCH, r#"{"id":"app.b"}"#)
    );
    r.set_reply(OK, 5130);
    r.tick(5700);
    assert_eq!(
        r.sel(e::TV_APP).as_deref(),
        Some("Other"),
        "not yet in the foreground"
    );
    r.tv.fg = "app.b".to_owned();
    r.tick(8300);
    assert_eq!(r.sel(e::TV_APP).as_deref(), Some("App B"));
    assert!(r.c.check.is_none(), "matched on the second read");
}

#[test]
fn other_and_unknown_options_are_refused_and_the_state_is_sent_again() {
    let mut r = Rig::boot(all_on(), Tv::new());
    r.states.remove(e::TV_APP);
    r.cmd(sel_cmd(e::TV_APP, "Other"), 1000);
    assert!(r.sets.is_empty());
    assert_eq!(
        r.sel(e::TV_APP).as_deref(),
        Some("App A"),
        "the real state is sent again"
    );
    r.states.remove(e::TV_INPUT);
    r.cmd(sel_cmd(e::TV_INPUT, "Other"), 2000)
        .cmd(sel_cmd(e::TV_INPUT, "HDMI 9"), 2100);
    assert!(r.sets.is_empty());
    assert_eq!(r.sel(e::TV_INPUT).as_deref(), Some("Other"));
    r.cmd(sel_cmd(e::TV_APP, "Voice Satellite"), 3000);
    assert!(r.sets.is_empty(), "the own app is not in the list");
}

#[test]
fn input_and_app_changes_are_two_seconds_apart() {
    let mut r = Rig::boot(all_on(), Tv::new());
    r.cmd(sel_cmd(e::TV_APP, "App B"), 1000);
    r.set_reply(OK, 1100);
    r.tv.fg = "app.b".to_owned();
    r.tick(1700);
    r.cmd(sel_cmd(e::TV_APP, "App A"), 1900);
    assert_eq!(r.sets.len(), 1);
    assert!(r.logged("one per 2 s"));
    r.cmd(sel_cmd(e::TV_APP, "App A"), 3100);
    assert_eq!(r.sets.len(), 2);
}

#[test]
fn all_setters_together_are_limited_to_thirty_per_minute() {
    let mut r = Rig::boot(all_on(), Tv::new());
    for n in 0..40_u64 {
        let on = n % 2 == 0;
        // Alternate so every command does something.
        r.tv.muted = !on;
        r.c.vol = r.c.vol.take().map(|mut v| {
            v.muted = !on;
            v
        });
        r.cmd(sw_cmd(e::TV_MUTE, on), 1000 + n * 100);
        if r.sets.len() as u64 > n {
            r.set_reply(OK, 1000 + n * 100 + 10);
            r.c.check = None;
        }
    }
    assert_eq!(r.sets.len(), 30);
    assert!(r.logged("30 per minute"));
}

// ----- capability detection (design 6) --------------------------------------

/// Boots with a fake TV whose answer for `topic` is `reply`.
fn boot_with(cfg: Config, topic: Topic, reply: Result<&str, &str>) -> Rig {
    let mut c = Controller::new(cfg);
    declare_all(&mut c);
    let mut r = Rig {
        c,
        tv: Tv::new(),
        t0: Instant::now(),
        sets: Vec::new(),
        reads: Vec::new(),
        logs: Vec::new(),
        states: HashMap::new(),
        own_wakes: 0,
    };
    let fx = r.c.start();
    for f in fx {
        let Effect::Call {
            purpose: Purpose::Read(t),
            ..
        } = f
        else {
            panic!()
        };
        let result = if t == topic {
            reply.map(str::to_owned).map_err(str::to_owned)
        } else {
            Ok(r.tv.answer(t))
        };
        let fx = r.c.on_reply(Purpose::Read(t), result, r.t0);
        r.run(fx);
    }
    let fx = r.c.finish_probe(r.t0);
    r.run(fx);
    let fx = r.c.set_env(env(true, false, Ok(())));
    r.run(fx);
    r
}

#[test]
fn a_failed_getter_gives_a_missing_state_and_refused_commands_with_one_log_line() {
    for (topic, bad) in [
        (Topic::Volume, Err("no answer within 5 s")),
        (
            Topic::Volume,
            Ok(r#"{"returnValue":false,"errorCode":-1,"errorText":"Denied"}"#),
        ),
        (
            Topic::Volume,
            Ok(r#"{"returnValue":true,"volumeStatus":{"volume":3}}"#),
        ),
        (Topic::Volume, Ok("")),
        (Topic::Volume, Ok("garbage")),
    ] {
        let mut r = boot_with(all_on(), topic, bad);
        assert_eq!(r.num(e::TV_VOLUME), None, "{bad:?}");
        assert_eq!(r.sw(e::TV_MUTE), None);
        assert_eq!(r.text(e::TV_SOUND_OUTPUT), None);
        // The other controls are unaffected.
        assert_eq!(r.sel(e::TV_APP).as_deref(), Some("App A"));
        let warns = r
            .logs
            .iter()
            .filter(|(l, m)| *l == Level::Warn && m.contains("cannot read the volume"))
            .count();
        assert_eq!(warns, 1, "one clear log line: {:?}", r.logs);
        r.cmd(num_cmd(e::TV_VOLUME, 20.0), 1000)
            .cmd(sw_cmd(e::TV_MUTE, true), 1100)
            .tick(2000);
        assert!(r.sets.is_empty(), "never guessed: {bad:?}");
        // The summary says so.
        assert!(r.logged("volume=no state"));
    }
}

#[test]
fn each_getter_failure_only_affects_its_controls() {
    let r = boot_with(all_on(), Topic::Inputs, Err("boom"));
    assert_eq!(r.sel(e::TV_INPUT), None);
    assert_eq!(r.num(e::TV_VOLUME), Some(12.0));
    assert_eq!(r.sel(e::TV_APP).as_deref(), Some("App A"));
    // The foreground app feeds both selects.
    let r = boot_with(all_on(), Topic::Foreground, Err("boom"));
    assert_eq!(r.sel(e::TV_INPUT), None);
    assert_eq!(r.sel(e::TV_APP), None);
    assert_eq!(r.num(e::TV_VOLUME), Some(12.0));
    let r = boot_with(all_on(), Topic::Apps, Ok("{}"));
    assert_eq!(r.sel(e::TV_APP), None);
    let r = boot_with(all_on(), Topic::PlayState, Err("boom"));
    assert_eq!(r.text(e::TV_PLAY_STATE), None);
    // Screen and power state still work with every getter broken.
    let r = boot_with(all_on(), Topic::Power, Err("boom"));
    assert_eq!(
        r.sw(e::TV_SCREEN),
        None,
        "no power state: unknown, never guessed"
    );
}

#[test]
fn a_probe_that_runs_out_of_time_counts_as_failed() {
    let mut c = Controller::new(all_on());
    declare_all(&mut c);
    let fx = c.start();
    let t0 = Instant::now();
    // Only the power state and the foreground app answer.
    for f in fx {
        let Effect::Call {
            purpose: Purpose::Read(t),
            ..
        } = f
        else {
            panic!()
        };
        match t {
            Topic::Power => {
                c.on_reply(Purpose::Read(t), Ok(power("Active")), t0);
            }
            Topic::Foreground => {
                c.on_reply(Purpose::Read(t), Ok(fg("app.a")), t0);
            }
            _ => {}
        }
    }
    assert!(!c.probe_complete());
    let fx = c.finish_probe(t0);
    assert!(c.probe_complete());
    let warns = fx
        .iter()
        .filter(|f| matches!(f, Effect::Log(Level::Warn, m) if m.contains("no answer in time")))
        .count();
    assert_eq!(
        warns, 6,
        "volume, inputs, apps, play state and the two LG settings"
    );
    assert!(!fx.iter().any(|f| matches!(f, Effect::Call { .. })));
}

#[test]
fn an_undeclared_setter_makes_the_control_read_only() {
    let mut c = Controller::new(all_on());
    for s in Setter::ALL {
        c.set_declared(s, Declared::Yes);
    }
    c.set_declared(Setter::Mute, Declared::No);
    c.set_declared(Setter::Input, Declared::Unchecked);
    let mut r = Rig::boot(all_on(), Tv::new());
    r.c = c;
    // Re-boot the controller the rig now holds.
    let fx = r.c.start();
    r.run(fx);
    let fx = r.c.finish_probe(r.t0);
    r.run(fx);
    r.c.set_env(env(true, false, Ok(())));
    assert!(r.logged("setMute is not declared by the TV"));
    assert!(r.logged("cannot check that the TV declares switchInput"));
    // The state is still shown, the writes are refused.
    assert_eq!(r.sw(e::TV_MUTE), Some(false));
    r.cmd(sw_cmd(e::TV_MUTE, true), 1000);
    assert!(r.sets.is_empty());
    assert!(r.logged("does not declare setMute"));
    // Unchecked is allowed (the read-back is the guard).
    r.cmd(sel_cmd(e::TV_INPUT, "HDMI 2"), 2000);
    assert_eq!(r.sets.len(), 1);
    // Nothing declared at all (never told): every untested setter is refused.
    let mut c = Controller::new(all_on());
    let mut r = Rig::boot(all_on(), Tv::new());
    c.set_env(env(true, false, Ok(())));
    r.c = c;
    let fx = r.c.start();
    r.run(fx);
    let fx = r.c.finish_probe(r.t0);
    r.run(fx);
    r.cmd(btn(e::TV_VOLUME_UP), 1000)
        .cmd(sel_cmd(e::TV_APP, "App B"), 1100);
    assert!(r.sets.is_empty());
}

#[test]
fn the_firmware_guard_blocks_setters_but_not_getters() {
    let cfg = Config {
        setters_blocked: Some("firmware not validated".to_owned()),
        ..all_on()
    };
    let mut r = Rig::boot(cfg, Tv::new());
    // Sensors and states report.
    assert_eq!(r.num(e::TV_VOLUME), Some(12.0));
    assert_eq!(r.text(e::TV_SOUND_OUTPUT).as_deref(), Some("external_arc"));
    assert_eq!(r.sw(e::TV_SCREEN), Some(true));
    assert_eq!(r.sel(e::TV_APP).as_deref(), Some("App A"));
    assert_eq!(
        r.sw(e::TV_POWER),
        None,
        "tv_power is unknown under the guard"
    );
    assert!(r.logged("setters off: firmware not validated"));
    for c in [
        sw_cmd(e::TV_SCREEN, false),
        btn(e::TV_TURN_ON),
        sw_cmd(e::TV_MUTE, true),
        btn(e::TV_VOLUME_UP),
        sel_cmd(e::TV_INPUT, "HDMI 2"),
        sel_cmd(e::TV_APP, "App B"),
        sw_cmd(e::TV_POWER, false),
    ] {
        r.cmd(c, 1000);
    }
    r.power("ScreenOff", false, 2000)
        .cmd(btn(e::TV_TURN_ON), 3000);
    r.cmd(num_cmd(e::TV_VOLUME, 20.0), 4000).tick(5000);
    assert!(r.sets.is_empty(), "{:?}", r.set_names());
}

// ----- polling, subscriptions, resume, lists --------------------------------

#[test]
fn polling_runs_in_the_right_states_until_a_subscription_is_established() {
    let mut r = Rig::boot(all_on(), Tv::new());
    let reads = |r: &mut Rig, at: u64| {
        let fx = r.c.tick(ms(r.t0, at));
        fx.iter()
            .filter(|f| {
                matches!(
                    f,
                    Effect::Call {
                        purpose: Purpose::Read(_),
                        ..
                    }
                )
            })
            .count()
    };
    // Nothing before the interval, then volume, foreground and play state.
    assert_eq!(reads(&mut r, 3000), 0);
    r.tv.vol = 14;
    r.tick(10_100);
    assert_eq!(r.num(e::TV_VOLUME), Some(14.0), "polled");
    // The foreground app polls every 5 s, the others every 10 s.
    r.tv.fg = "app.b".to_owned();
    r.tick(15_200);
    assert_eq!(r.sel(e::TV_APP).as_deref(), Some("App B"));
    // None in Active Standby.
    r.power("ActiveStandby", false, 16_000);
    assert_eq!(reads(&mut r, 40_000), 0);
    // The play state only in Active and Screen Saver.
    r.power("ScreenOff", false, 41_000);
    let fx = r.c.tick(ms(r.t0, 60_000));
    assert!(fx.iter().all(|f| !matches!(
        f,
        Effect::Call {
            purpose: Purpose::Read(Topic::PlayState),
            ..
        }
    )));
    // A working subscription ends the polling of that topic.
    r.power("Active", false, 61_000);
    let line = r#"{"returnValue":true,"subscribed":true,"volumeStatus":{"volume":20,"maxVolume":60,"muteStatus":false,"soundOutput":"external_arc"}}"#;
    let fx =
        r.c.on_subscription_line(Topic::Volume, line, ms(r.t0, 61_100));
    r.run(fx);
    assert_eq!(r.num(e::TV_VOLUME), Some(20.0));
    let n =
        r.c.tick(ms(r.t0, 100_000))
            .iter()
            .filter(|f| {
                matches!(
                    f,
                    Effect::Call {
                        purpose: Purpose::Read(Topic::Volume),
                        ..
                    }
                )
            })
            .count();
    assert_eq!(n, 0, "subscribed: no polling");
}

#[test]
fn a_subscription_without_the_flag_is_kept_and_polled_until_a_push_proves_it() {
    let reads_of = |r: &mut Rig, t: Topic, at: u64| {
        r.c.tick(ms(r.t0, at))
            .iter()
            .filter(|f| matches!(f, Effect::Call { purpose: Purpose::Read(x), .. } if *x == t))
            .count()
    };
    let mut r = Rig::boot(all_on(), Tv::new());
    let fx =
        r.c.on_subscription_line(Topic::Foreground, &fg("app.b"), ms(r.t0, 100));
    assert!(
        !fx.iter().any(|f| matches!(f, Effect::Unsubscribe(_))),
        "the subscription stays"
    );
    assert!(
        fx.iter()
            .any(|f| matches!(f, Effect::Log(Level::Info, m) if m.contains("no subscribed flag")))
    );
    r.run(fx);
    assert_eq!(
        r.sel(e::TV_APP).as_deref(),
        Some("App B"),
        "the line is still used"
    );
    // Polling goes on while it is unconfirmed.
    assert_eq!(reads_of(&mut r, Topic::Foreground, 6000), 1);
    // A second line is a push: the subscription streams, polling stops.
    let fx =
        r.c.on_subscription_line(Topic::Foreground, &fg("app.a"), ms(r.t0, 7000));
    assert!(
        fx.iter()
            .any(|f| matches!(f, Effect::Log(Level::Info, m) if m.contains("pushed")))
    );
    assert!(!fx.iter().any(|f| matches!(f, Effect::Unsubscribe(_))));
    r.run(fx);
    assert_eq!(r.sel(e::TV_APP).as_deref(), Some("App A"));
    assert_eq!(reads_of(&mut r, Topic::Foreground, 30_000), 0);
    assert_eq!(reads_of(&mut r, Topic::Foreground, 60_000), 0);
}

#[test]
fn a_silent_subscription_is_dropped_when_polling_sees_a_change_it_did_not_push() {
    // The rig answers reads at its start time; here a poll is answered at
    // the time it is due.
    let poll = |r: &mut Rig, at: u64| {
        for f in r.c.tick(ms(r.t0, at)) {
            if let Effect::Call {
                purpose: Purpose::Read(t),
                ..
            } = f
            {
                let text = r.tv.answer(t);
                let fx = r.c.on_reply(Purpose::Read(t), Ok(text), ms(r.t0, at));
                r.run(fx);
            }
        }
    };
    let mut r = Rig::boot(all_on(), Tv::new());
    let first = r#"{"returnValue":true,"volumeStatus":{"volume":12,"maxVolume":60,"muteStatus":false,"soundOutput":"external_arc"}}"#;
    let fx =
        r.c.on_subscription_line(Topic::Volume, first, ms(r.t0, 100));
    r.run(fx);
    // The volume changes at the TV and only the poll sees it.
    r.tv.vol = 20;
    poll(&mut r, 10_100);
    assert_eq!(r.num(e::TV_VOLUME), Some(20.0));
    // Within the grace period nothing is dropped.
    let fx = r.c.tick(ms(r.t0, 11_000));
    assert!(!fx.iter().any(|f| matches!(f, Effect::Unsubscribe(_))));
    // After it: polling only, with a warning.
    let fx = r.c.tick(ms(r.t0, 13_200));
    assert!(
        fx.iter()
            .any(|f| matches!(f, Effect::Unsubscribe(Topic::Volume)))
    );
    assert!(
        fx.iter()
            .any(|f| matches!(f, Effect::Log(Level::Warn, m) if m.contains("pushed nothing")))
    );
    // Polling continues.
    r.tv.vol = 21;
    poll(&mut r, 20_300);
    assert_eq!(r.num(e::TV_VOLUME), Some(21.0));
    // A change that is pushed within the grace period keeps the subscription.
    let mut r = Rig::boot(all_on(), Tv::new());
    let fx =
        r.c.on_subscription_line(Topic::Volume, first, ms(r.t0, 100));
    r.run(fx);
    r.tv.vol = 20;
    poll(&mut r, 10_100);
    let push = r#"{"returnValue":true,"volumeStatus":{"volume":20,"maxVolume":60,"muteStatus":false,"soundOutput":"external_arc"}}"#;
    let fx =
        r.c.on_subscription_line(Topic::Volume, push, ms(r.t0, 11_000));
    r.run(fx);
    let fx = r.c.tick(ms(r.t0, 14_000));
    assert!(!fx.iter().any(|f| matches!(f, Effect::Unsubscribe(_))));
    let n =
        r.c.tick(ms(r.t0, 40_000))
            .iter()
            .filter(|f| {
                matches!(
                    f,
                    Effect::Call {
                        purpose: Purpose::Read(Topic::Volume),
                        ..
                    }
                )
            })
            .count();
    assert_eq!(n, 0, "confirmed by the push: no polling");
}

#[test]
fn a_subscription_that_answers_with_an_error_falls_back_to_polling() {
    let mut r = Rig::boot(all_on(), Tv::new());
    let fx = r.c.on_subscription_line(
        Topic::Volume,
        r#"{"returnValue":false,"errorText":"x"}"#,
        ms(r.t0, 100),
    );
    assert!(
        fx.iter()
            .any(|f| matches!(f, Effect::Unsubscribe(Topic::Volume)))
    );
}

#[test]
fn startup_asks_for_subscriptions_of_volume_and_foreground_only() {
    let mut c = Controller::new(all_on());
    declare_all(&mut c);
    let t0 = Instant::now();
    let tv = Tv::new();
    for f in c.start() {
        let Effect::Call {
            purpose: Purpose::Read(t),
            ..
        } = f
        else {
            panic!()
        };
        c.on_reply(Purpose::Read(t), Ok(tv.answer(t)), t0);
    }
    let subs: Vec<Topic> = c
        .finish_probe(t0)
        .into_iter()
        .filter_map(|f| {
            if let Effect::Subscribe(t) = f {
                Some(t)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(subs, [Topic::Volume, Topic::Foreground]);
    assert_eq!(
        Topic::Volume.subscription_uri(),
        Some(setters::URI_GET_VOLUME)
    );
    assert_eq!(Topic::Apps.subscription_uri(), None);
}

#[test]
fn resume_reads_everything_before_any_state_is_sent() {
    let mut r = Rig::boot(all_on(), Tv::new());
    r.tv.vol = 30;
    let fx = r.c.on_resume(ms(r.t0, 5000));
    let reads = fx
        .iter()
        .filter(|f| {
            matches!(
                f,
                Effect::Call {
                    purpose: Purpose::Read(_),
                    ..
                }
            )
        })
        .count();
    assert_eq!(reads, 8, "six getters and the two LG settings");
    // While the reads are out nothing is sent.
    let mut states_before = 0;
    for f in &fx {
        if matches!(f, Effect::State(_)) {
            states_before += 1;
        }
    }
    assert_eq!(states_before, 0);
    r.states.clear();
    r.run(fx);
    assert_eq!(r.num(e::TV_VOLUME), Some(30.0));
    assert!(
        r.states.len() >= 8,
        "every state is announced again after the reads: {}",
        r.states.len()
    );
    assert!(r.logged("resumed"));
    // A resume before the startup check is ignored.
    let mut c = Controller::new(all_on());
    assert!(c.on_resume(Instant::now()).is_empty());
}

#[test]
fn back_from_standby_reads_the_values_again() {
    let mut r = Rig::boot(all_on(), Tv::new());
    r.power("ActiveStandby", false, 1000);
    r.tv.vol = 22;
    r.tv.fg = "app.b".to_owned();
    r.power("Active", false, 9000);
    assert_eq!(r.num(e::TV_VOLUME), Some(22.0));
    assert_eq!(r.sel(e::TV_APP).as_deref(), Some("App B"));
}

#[test]
fn refresh_reads_the_lists_and_a_change_waits_for_the_daemon() {
    let mut r = Rig::boot(all_on(), Tv::new());
    assert!(r.c.pending_options().is_empty());
    // An app was installed.
    r.tv.points = POINTS.replace(
        r#"{"id":"app.b","title":"App B"}"#,
        r#"{"id":"app.b","title":"App B"},{"id":"app.c","title":"App C"}"#,
    );
    r.cmd(btn(e::TV_REFRESH_LISTS), 10_000);
    assert!(r.logged("the app list changed (3 entries)"));
    assert!(
        r.logged("tv_refresh_lists: done: 2 inputs, 3 apps"),
        "{:?}",
        r.logs
    );
    assert_eq!(
        r.c.pending_options(),
        [(
            e::TV_APP,
            vec![
                "App A".to_owned(),
                "App B".into(),
                "App C".into(),
                "Other".into()
            ]
        )]
    );
    // Until the daemon applied it, HA still sees the old options and the
    // state stays inside them.
    r.tv.fg = "app.c".to_owned();
    r.tick(16_000);
    assert_eq!(r.sel(e::TV_APP).as_deref(), Some("Other"));
    // The daemon applies the options (HA lists again): the state follows.
    let fx = r.c.options_applied(e::TV_APP);
    r.run(fx);
    assert_eq!(r.sel(e::TV_APP).as_deref(), Some("App C"));
    assert!(r.c.pending_options().is_empty());
    let Some(Kind::Select { options }) =
        r.c.entities()
            .into_iter()
            .find(|e| e.id == e::TV_APP)
            .map(|e| e.kind)
    else {
        panic!()
    };
    assert_eq!(options.len(), 4);
    // Refreshing again straight away is refused; unchanged lists change nothing.
    r.cmd(btn(e::TV_REFRESH_LISTS), 10_500);
    assert!(r.logged("refreshed a moment ago"));
    r.cmd(btn(e::TV_REFRESH_LISTS), 30_000);
    assert!(r.c.pending_options().is_empty());
    // A list that changes back before it was applied cancels the pending options.
    r.tv.points = POINTS.to_owned();
    r.cmd(btn(e::TV_REFRESH_LISTS), 40_000);
    assert_eq!(r.c.pending_options().len(), 1);
    r.tv.points = POINTS.replace(
        r#"{"id":"app.b","title":"App B"}"#,
        r#"{"id":"app.b","title":"App B"},{"id":"app.c","title":"App C"}"#,
    );
    r.cmd(btn(e::TV_REFRESH_LISTS), 50_000);
    assert!(r.c.pending_options().is_empty());
}

#[test]
fn a_failed_list_refresh_keeps_the_old_lists() {
    let mut r = Rig::boot(all_on(), Tv::new());
    // A reply over the size limit is refused, with no body in the log.
    let big = format!(
        r#"{{"returnValue":true,"launchPoints":[],"pad":"{}"}}"#,
        "x".repeat(luna::MAX_REPLY_BYTES)
    );
    let fx =
        r.c.on_reply(Purpose::Read(Topic::Apps), Ok(big), ms(r.t0, 5000));
    r.run(fx);
    assert!(r.logged("over the 1048576 byte limit"));
    assert!(!r.logs.iter().any(|(_, m)| m.contains("xxxx")));
    let fx = r.c.on_command(btn(e::TV_REFRESH_LISTS), ms(r.t0, 10_000));
    for f in fx {
        if let Effect::Call { purpose, .. } = f {
            let fx = r.c.on_reply(
                purpose,
                Err("no answer within 5 s".to_owned()),
                ms(r.t0, 10_100),
            );
            r.run(fx);
        }
    }
    assert!(r.c.pending_options().is_empty());
    assert!(
        r.logs
            .iter()
            .any(|(l, m)| *l == Level::Warn && m.contains("tv_refresh_lists: failed to read")),
        "{:?}",
        r.logs
    );
    assert!(!r.logged("tv_refresh_lists: done"));
    assert_eq!(
        r.c.entities()
            .iter()
            .filter(|e| matches!(e.kind, Kind::Select { .. }))
            .count(),
        2
    );
    let Some(Kind::Select { options }) =
        r.c.entities()
            .into_iter()
            .find(|e| e.id == e::TV_APP)
            .map(|e| e.kind)
    else {
        panic!()
    };
    assert_eq!(options, ["App A", "App B", "Other"], "the old list stays");
}

// ----- privacy -------------------------------------------------------------------

/// No app title, app id, input label or reply body reaches `info` or `warn`.
#[test]
fn nothing_the_owner_watches_is_logged_above_debug() {
    let canaries = [
        "CANARY-TITLE",
        "canary.app.id",
        "CANARY-LABEL",
        "CANARY_INPUT",
        "SECRET-TOKEN",
    ];
    let mut tv = Tv::new();
    tv.inputs = r#"{"returnValue":true,"devices":[
        {"id":"CANARY_INPUT","label":"CANARY-LABEL","connected":true,"appId":"canary.input.app"}]}"#
        .to_owned();
    tv.points = r#"{"returnValue":true,"launchPoints":[
        {"id":"canary.app.id","title":"CANARY-TITLE","userData":{"t":"SECRET-TOKEN"}},
        {"id":"app.b","title":"App B"}]}"#
        .to_owned();
    tv.fg = "canary.app.id".to_owned();
    let mut r = Rig::boot(all_on(), tv);
    assert_eq!(
        r.sel(e::TV_APP).as_deref(),
        Some("CANARY-TITLE"),
        "the state itself goes to HA"
    );
    r.cmd(sel_cmd(e::TV_APP, "CANARY-TITLE"), 1000);
    r.set_reply(OK, 1100);
    r.tick(1700);
    r.cmd(sel_cmd(e::TV_INPUT, "CANARY-LABEL"), 5000);
    r.set_reply(
        r#"{"returnValue":false,"errorCode":-1000,"errorText":"no such input"}"#,
        5100,
    );
    // A reply with a body that must not be logged.
    let fx = r.c.on_reply(
        Purpose::Read(Topic::Volume),
        Ok(r#"{"returnValue":true,"volumeStatus":{"serial":"SECRET-TOKEN"}}"#.to_owned()),
        ms(r.t0, 9000),
    );
    r.run(fx);
    let fx = r.c.on_reply(
        Purpose::Read(Topic::Apps),
        Ok("not json SECRET-TOKEN".to_owned()),
        ms(r.t0, 9100),
    );
    r.run(fx);
    r.cmd(btn(e::TV_REFRESH_LISTS), 20_000);
    for (level, line) in &r.logs {
        if *level == Level::Debug {
            continue;
        }
        for c in canaries {
            assert!(!line.contains(c), "{level:?} line leaks {c}: {line}");
        }
    }
    assert!(!format!("{:?}", Action::Launch("canary.app.id".into())).contains("canary"));
    assert!(
        !format!("{:?}", Purpose::Set(Action::Input("CANARY_INPUT".into()))).contains("CANARY")
    );
}

#[test]
fn unknown_entities_are_ignored() {
    let off = Config {
        volume: false,
        mute: false,
        input: false,
        apps: false,
        ..Config::default()
    };
    let mut r = Rig::boot(off, Tv::new());
    r.cmd(sw_cmd("nope", true), 100)
        .cmd(btn(e::TV_VOLUME_UP), 200)
        .cmd(sw_cmd(e::TV_MUTE, true), 300);
    r.cmd(num_cmd(e::TV_VOLUME, 5.0), 400)
        .cmd(sel_cmd(e::TV_APP, "x"), 500)
        .tick(1000);
    assert!(r.sets.is_empty(), "controls that are off are not reachable");
}

#[test]
fn power_state_updates_with_processing_only_keep_the_state() {
    let mut r = Rig::boot(all_on(), Tv::new());
    r.power("", true, 1000);
    assert_eq!(r.sw(e::TV_SCREEN), Some(true));
    r.power("", false, 2000);
    assert_eq!(r.sw(e::TV_SCREEN), Some(true));
    assert_eq!(PowerState::from_key("ScreenSaver"), PowerState::ScreenSaver);
    assert_eq!(PowerState::from_key("Weird"), PowerState::Other);
}

// ----- the TV media player (ADR 0007) ----------------------------------------

fn mp(action: MediaAction) -> Command {
    Command::Media {
        id: e::TV_MEDIA_PLAYER.to_owned(),
        action,
    }
}

#[test]
fn media_volume_share_and_steps() {
    assert_eq!(volume_share(12, 60), Some(0.2));
    assert_eq!(volume_share(0, 60), Some(0.0));
    assert_eq!(volume_share(70, 60), Some(1.0), "never above 1");
    assert_eq!(volume_share(3, 0), None, "no maximum, no share");
    assert!((share_to_steps(0.2, 60) - 12.0).abs() < 1e-4);
    assert!((share_to_steps(1.7, 60) - 60.0).abs() < 1e-4);
    assert!(share_to_steps(-1.0, 60).abs() < 1e-6);
    // Every TV value survives the round trip HA makes (share, then steps).
    for max in [60_u16, 100] {
        for v in 0..=max {
            let (vv, mm) = (u32::from(v), u32::from(max));
            let back = share_to_steps(volume_share(vv, mm).unwrap(), mm).round();
            assert!((back - f32::from(v)).abs() < 1e-6, "{v}/{max}");
        }
    }
}

#[test]
fn media_player_state_follows_power_play_state_volume_and_mute() {
    let mut r = Rig::boot(all_on(), Tv::new());
    // Active and the media service says playing: playing, 12 of 60.
    assert_eq!(
        r.media(),
        MediaState {
            power: MediaPower::Playing,
            volume: Some(0.2),
            muted: false
        }
    );
    // Not playing: on.
    r.tv.play = r#"{"returnValue":true,"playState":"paused"}"#.to_owned();
    r.tv.muted = true;
    r.tick(10_100);
    assert_eq!(
        r.media(),
        MediaState {
            power: MediaPower::On,
            volume: Some(0.2),
            muted: true
        }
    );
    // A transition holds the last state.
    r.power("Active", true, 11_000);
    assert_eq!(r.media().power, MediaPower::On);
    // Screen Off looks off; volume and mute are still live there.
    r.power("ScreenOff", false, 12_000);
    assert_eq!(
        r.media(),
        MediaState {
            power: MediaPower::Off,
            volume: Some(0.2),
            muted: true
        }
    );
    // Lit again: the play state is read again at once.
    r.tv.play = PLAY.to_owned();
    r.power("Active", false, 13_000);
    assert_eq!(r.media().power, MediaPower::Playing);
    // Active Standby: off, no volume known.
    r.power("ActiveStandby", false, 14_000);
    assert_eq!(
        r.media(),
        MediaState {
            power: MediaPower::Off,
            volume: None,
            muted: false
        }
    );
    // An unknown power state is held (no guess, and no "unknown" exists).
    r.power("Weird", false, 15_000);
    assert_eq!(r.media().power, MediaPower::Off);
}

#[test]
fn media_player_without_a_volume_reading_shows_no_volume_and_refuses_sets() {
    let mut r = boot_with(
        all_on(),
        Topic::Volume,
        Ok(r#"{"returnValue":false,"errorCode":-1,"errorText":"Denied"}"#),
    );
    let m = r.media();
    assert_eq!(
        (m.power, m.volume, m.muted),
        (MediaPower::Playing, None, false)
    );
    r.cmd(mp(MediaAction::SetVolume(0.5)), 1000)
        .cmd(mp(MediaAction::VolumeUp), 1100)
        .cmd(mp(MediaAction::Mute(true)), 1200)
        .tick(2000);
    assert!(r.sets.is_empty(), "{:?}", r.set_names());
    assert!(r.logged("refused"));
    // Without the play state getter the media player is just on.
    let r = boot_with(
        all_on(),
        Topic::PlayState,
        Ok(r#"{"returnValue":false,"errorCode":-1,"errorText":"Denied"}"#),
    );
    assert_eq!(r.media().power, MediaPower::On);
}

#[test]
fn media_player_commands_use_the_same_setters_limits_and_read_backs() {
    let mut r = Rig::boot(all_on(), Tv::new());
    // Volume set: share of maxVolume (60), rounded, coalesced, read back.
    r.cmd(mp(MediaAction::SetVolume(0.5)), 1000).tick(1300);
    assert_eq!(
        (r.sets[0].1, r.sets[0].2.as_str()),
        (setters::URI_SET_VOLUME, r#"{"volume":30}"#)
    );
    r.set_reply(r#"{"returnValue":true,"volume":30,"soundOutput":""}"#, 1350);
    r.tv.vol = 30;
    r.tick(1900);
    assert_eq!(r.media().volume, Some(0.5));
    assert_eq!(r.num(e::TV_VOLUME), Some(30.0), "the number follows too");
    assert!(r.logged("read back matches"));
    // A share that is not a whole step rounds to the nearest one.
    r.cmd(mp(MediaAction::SetVolume(0.209)), 5000).tick(5300);
    assert_eq!(r.sets[1].2, r#"{"volume":13}"#);
    r.set_reply(OK, 5350);
    r.tv.vol = 13;
    r.tick(5900);
    // Volume up and down: one TV step each, the remote's methods.
    r.cmd(mp(MediaAction::VolumeUp), 7000);
    assert_eq!(r.sets[2].1, setters::URI_VOLUME_UP);
    r.set_reply(OK, 7050);
    r.tv.vol = 14;
    r.tick(7600);
    r.cmd(mp(MediaAction::VolumeDown), 8000);
    assert_eq!(r.sets[3].1, setters::URI_VOLUME_DOWN);
    r.set_reply(OK, 8050);
    r.tv.vol = 13;
    r.tick(8600);
    // Mute and unmute: the verified setter.
    r.cmd(mp(MediaAction::Mute(true)), 9000);
    assert_eq!(
        (r.sets[4].1, r.sets[4].2.as_str()),
        (setters::URI_SET_MUTE, r#"{"mute":true}"#)
    );
    r.set_reply(OK, 9050);
    r.tv.muted = true;
    r.tick(9600);
    assert!(r.media().muted);
    assert_eq!(r.sw(e::TV_MUTE), Some(true));
    // Already muted: nothing to do.
    r.cmd(mp(MediaAction::Mute(true)), 12_000);
    assert_eq!(r.sets.len(), 5);
    // The step rate limit is shared with the buttons (4 per second).
    let n = r.sets.len();
    for i in 0..6 {
        r.cmd(mp(MediaAction::VolumeDown), 20_000 + i * 10);
        let fx = r.c.on_reply(
            Purpose::Set(Action::VolumeDown),
            Ok(OK.to_owned()),
            ms(r.t0, 20_000 + i * 10 + 1),
        );
        r.run(fx);
    }
    assert!(r.sets.len() - n <= 4, "{}", r.sets.len() - n);
}

#[test]
fn media_player_volume_follows_the_volume_limit() {
    let mut tv = Tv::new();
    tv.vol = 50;
    let mut r = Rig::boot(
        Config {
            volume_limit: Some(40),
            ..all_on()
        },
        tv,
    );
    // The share is of maxVolume, not of the cap: 50 of 60 shows as 0.83.
    assert!((r.media().volume.unwrap() - 50.0 / 60.0).abs() < 1e-6);
    // Full volume is clamped to the cap (logged); up is refused above it;
    // down still works.
    r.cmd(mp(MediaAction::VolumeUp), 1000);
    assert!(r.sets.is_empty());
    r.cmd(mp(MediaAction::SetVolume(1.0)), 2000).tick(2300);
    assert_eq!(r.sets[0].2, r#"{"volume":40}"#);
    assert!(r.logged("volume ceiling"));
}

#[test]
fn media_player_turn_off_is_the_power_key_when_safe_else_the_screen() {
    // Keep listening: the remote's key, power_off enabled or not (the
    // satellite wakes the TV and darkens it).
    for power_off in [true, false] {
        let mut r = Rig::boot(
            Config {
                power_off,
                ..all_on()
            },
            Tv::new(),
        );
        r.cmd(mp(MediaAction::TurnOff), 1000);
        assert_eq!(r.set_names(), ["power key"], "power_off = {power_off}");
        assert!(r.logged("tv_media_player: turn off: the remote's power key (\"Keep listening\""));
    }
    // Turn off normally with Always Ready on (both values), power_off off
    // (the default): the key, into LG's Active Standby.
    for word in ["alwaysReady", "allEnabled"] {
        let mut r = boot_normal(
            Config {
                power_off: false,
                ..all_on()
            },
            tv_always_ready(word),
        );
        r.cmd(mp(MediaAction::TurnOff), 1000);
        assert_eq!(r.set_names(), ["power key"], "{word}");
        assert_eq!(r.sets[0].1, setters::URI_POWER_KEY);
        assert!(r.logged("the TV goes to Active Standby"));
        r.set_reply(OK, 1050)
            .power("ActiveStandby", true, 1100)
            .power("ActiveStandby", false, 4000);
        assert_eq!(r.media().power, MediaPower::Off);
        // Already off: nothing; turn on is powerOn after the key block.
        r.cmd(mp(MediaAction::TurnOff), 4100);
        assert_eq!(r.sets.len(), 1, "already off");
        r.cmd(mp(MediaAction::TurnOn), 4200).tick(5600);
        assert_eq!(r.set_names(), ["power key", "powerOn"]);
    }
    // Turn off normally, Always Ready off (the fake TV's value) or unknown:
    // the screen off, with the reason in the log.
    for (cfg, why) in [
        (all_on(), "Always Ready is off"),
        (
            Config {
                lg_settings: false,
                ..all_on()
            },
            "Always Ready is not known",
        ),
    ] {
        let mut r = boot_normal(cfg, Tv::new());
        r.cmd(mp(MediaAction::TurnOff), 1000);
        assert_eq!(r.set_names(), ["turnOffScreen"], "{why}");
        assert!(r.logged("the screen off instead of the power key"));
        assert!(r.logged(why));
        r.set_reply(OK, 1050).power("ScreenOff", false, 1100);
        assert_eq!(r.media().power, MediaPower::Off);
        // Already off: nothing; turn on wakes the screen as the button does.
        r.cmd(mp(MediaAction::TurnOff), 3000);
        assert_eq!(r.sets.len(), 1);
        r.cmd(mp(MediaAction::TurnOn), 4000);
        assert_eq!(r.set_names(), ["turnOffScreen", "turnOnScreen"]);
        assert_eq!(r.own_wakes, 1);
    }
    // Always Ready off and the screen control off: refused, no key.
    let mut r = boot_normal(
        Config {
            screen: false,
            power_off: true,
            ..all_on()
        },
        Tv::new(),
    );
    r.cmd(mp(MediaAction::TurnOff), 1000);
    assert!(r.sets.is_empty());
    assert!(r.logged("refused"));
    // From Active Standby (whoever turned it off): nothing; turn on works.
    let mut r = Rig::boot(all_on(), Tv::new());
    r.power("ActiveStandby", false, 1000)
        .cmd(mp(MediaAction::TurnOff), 1100);
    assert!(r.sets.is_empty(), "already off");
    r.cmd(mp(MediaAction::TurnOn), 1200).tick(2600);
    assert_eq!(r.sets[0].1, setters::URI_POWER_ON);
}

#[test]
fn media_player_turn_off_gates_refuse_without_a_screen_fallback() {
    // A refusal that is not about Always Ready stays a refusal: no key and
    // no turnOffScreen.
    let blocked = Config {
        setters_blocked: Some("firmware".into()),
        ..all_on()
    };
    let cases: [(&str, Config, PowerOffEnv, bool, &str); 5] = [
        (
            "keep listening, darkener cooldown",
            all_on(),
            env(true, false, Err("cooldown")),
            false,
            "cooldown",
        ),
        (
            "keep listening, kill switch",
            all_on(),
            env(true, true, Ok(())),
            false,
            "kill switch",
        ),
        (
            "normal, kill switch",
            all_on(),
            env(false, true, Err("not keep listening")),
            false,
            "kill switch",
        ),
        (
            "normal, firmware guard",
            blocked,
            env(false, false, Err("not keep listening")),
            false,
            "setters are off",
        ),
        (
            "normal, transition in progress",
            all_on(),
            env(false, false, Err("not keep listening")),
            true,
            "changing power state",
        ),
    ];
    for (name, cfg, e, transition, why) in cases {
        let mut r = Rig::boot(cfg, tv_always_ready("alwaysReady"));
        let fx = r.c.set_env(e);
        r.run(fx);
        if transition {
            r.power("Active", true, 100);
        }
        r.cmd(mp(MediaAction::TurnOff), 1000).tick(2000);
        assert!(r.sets.is_empty(), "{name}: {:?}", r.set_names());
        assert!(r.logged(why), "{name}");
    }
    // One key at a time (also before its reply).
    let mut r = boot_normal(all_on(), tv_always_ready("alwaysReady"));
    r.cmd(mp(MediaAction::TurnOff), 1000)
        .cmd(mp(MediaAction::TurnOff), 1100);
    assert_eq!(r.set_names(), ["power key"]);
    assert!(r.logged("a power key was sent"));
}

#[test]
fn a_second_key_is_held_until_the_tv_reacts_or_the_hold_ends() {
    // The key's reply comes before the power state changes: a second key in
    // that gap would toggle the TV back on.
    let mut r = boot_normal(all_on(), tv_always_ready("alwaysReady"));
    r.cmd(mp(MediaAction::TurnOff), 1000).set_reply(OK, 1050);
    for c in [
        mp(MediaAction::TurnOff),
        mp(MediaAction::TurnOn),
        sw_cmd(e::TV_POWER, false),
        sw_cmd(e::TV_POWER, true),
    ] {
        r.cmd(c, 1300);
    }
    assert_eq!(r.set_names(), ["power key"]);
    assert!(
        r.logs
            .iter()
            .any(|(l, m)| *l == Level::Info && m.contains("refused: a power key was sent"))
    );
    // The TV reacts: the hold ends; back on from the settled standby.
    r.power("ActiveStandby", true, 1400)
        .power("ActiveStandby", false, 4000)
        .cmd(mp(MediaAction::TurnOn), 4100)
        .tick(5600);
    assert_eq!(r.set_names(), ["power key", "powerOn"]);
    // The TV never reacts (a lost key): allowed again after KEY_HOLD.
    let mut r = boot_normal(all_on(), tv_always_ready("alwaysReady"));
    r.cmd(mp(MediaAction::TurnOff), 1000).set_reply(OK, 1050);
    r.cmd(mp(MediaAction::TurnOff), 10_900);
    assert_eq!(r.sets.len(), 1, "still held");
    r.cmd(mp(MediaAction::TurnOff), 11_000);
    assert_eq!(r.set_names(), ["power key", "power key"]);
    // A change within the lit class (Active to Screen Saver) before the TV
    // reacts keeps the hold: a second key would still toggle the TV on.
    let mut r = boot_normal(all_on(), tv_always_ready("alwaysReady"));
    r.cmd(mp(MediaAction::TurnOff), 1000)
        .set_reply(OK, 1050)
        .power("ScreenSaver", false, 1200)
        .cmd(mp(MediaAction::TurnOff), 1300)
        .cmd(sw_cmd(e::TV_POWER, false), 1400);
    assert_eq!(r.set_names(), ["power key"]);
    assert!(r.logged("refused: a power key was sent"));
    // A key the TV refused does not hold.
    let mut r = boot_normal(all_on(), tv_always_ready("alwaysReady"));
    r.cmd(mp(MediaAction::TurnOff), 1000)
        .set_reply(r#"{"returnValue":false,"errorText":"x"}"#, 1050)
        .cmd(mp(MediaAction::TurnOff), 1300);
    assert_eq!(r.set_names(), ["power key", "power key"]);
}

#[test]
fn turn_off_decides_on_a_fresh_always_ready_read() {
    // Cached On (boot), the setting is now off: the fresh read wins, no key.
    let mut r = boot_normal(all_on(), tv_always_ready("alwaysReady"));
    assert_eq!(r.c.power_off_check(), Ok(PowerOffRoute::Standby));
    r.tv.always_ready = always_ready_reply("off");
    let reads = r.reads.len();
    r.cmd(mp(MediaAction::TurnOff), 1000);
    assert_eq!(r.reads.len(), reads + 1, "one read before the key");
    assert_eq!(r.reads[reads].0, Topic::AlwaysReady);
    assert_eq!(r.set_names(), ["turnOffScreen"]);
    assert!(r.logged("Always Ready is off"));
    assert_eq!(
        r.text(e::LG_ALWAYS_READY).as_deref(),
        Some("Off"),
        "the sensor's cache is updated as by the poll"
    );
    // The same for tv_power: refused, no fallback.
    let mut r = boot_normal(all_on(), tv_always_ready("alwaysReady"));
    r.tv.always_ready = always_ready_reply("off");
    r.cmd(sw_cmd(e::TV_POWER, false), 1000);
    assert!(r.sets.is_empty());
    // Cached Off, now on: the key.
    let mut r = boot_normal(all_on(), Tv::new());
    r.tv.always_ready = always_ready_reply("alwaysReady");
    r.cmd(mp(MediaAction::TurnOff), 1000);
    assert_eq!(r.set_names(), ["power key"]);
    // A failed read: unknown, the screen off.
    let mut r = boot_normal(all_on(), tv_always_ready("alwaysReady"));
    r.tv.always_ready = r#"{"returnValue":false,"errorText":"x"}"#.to_owned();
    r.cmd(mp(MediaAction::TurnOff), 1000);
    assert_eq!(r.set_names(), ["turnOffScreen"]);
    assert!(r.logged("Always Ready is not known"));
    // No answer within ALWAYS_READY_FRESH: unknown, the screen off; other
    // power commands wait meanwhile.
    let mut r = boot_normal(all_on(), tv_always_ready("alwaysReady"));
    let fx = r.c.on_command(mp(MediaAction::TurnOff), ms(r.t0, 1000));
    assert!(fx.iter().any(|f| matches!(
        f,
        Effect::Call {
            purpose: Purpose::Read(Topic::AlwaysReady),
            ..
        }
    )));
    // (The read is never answered.)
    r.cmd(sw_cmd(e::TV_POWER, false), 1500);
    assert!(r.logged("waiting for the Always Ready read"));
    r.tick(2900);
    assert!(r.sets.is_empty(), "still waiting");
    r.tick(3000);
    assert_eq!(r.set_names(), ["turnOffScreen"]);
    assert!(r.logged("did not answer within 2 s"));
    // A reply after the deadline (before the next tick) counts as no
    // answer, even if it says on.
    let mut r = boot_normal(all_on(), tv_always_ready("alwaysReady"));
    let _ = r.c.on_command(mp(MediaAction::TurnOff), ms(r.t0, 1000));
    let fx = r.c.on_reply(
        Purpose::Read(Topic::AlwaysReady),
        Ok(always_ready_reply("alwaysReady")),
        ms(r.t0, 3100),
    );
    r.run(fx);
    assert_eq!(r.set_names(), ["turnOffScreen"]);
    assert!(r.logged("did not answer within 2 s"));
    // Keep listening does not read Always Ready.
    let mut r = Rig::boot(all_on(), Tv::new());
    let reads = r.reads.len();
    r.cmd(mp(MediaAction::TurnOff), 1000);
    assert_eq!(r.reads.len(), reads);
    assert_eq!(r.set_names(), ["power key"]);
}

#[test]
fn a_read_issued_before_the_turn_off_is_not_taken_as_fresh() {
    let mut r = boot_normal(all_on(), tv_always_ready("alwaysReady"));
    // A refresh read is in flight (not answered yet) when the turn off comes.
    let fx = r.c.refresh_lg_settings(ms(r.t0, 900));
    assert!(fx.iter().any(|f| matches!(
        f,
        Effect::Call {
            purpose: Purpose::Read(Topic::AlwaysReady),
            ..
        }
    )));
    let fx = r.c.on_command(mp(MediaAction::TurnOff), ms(r.t0, 1000));
    assert!(
        !fx.iter().any(|f| matches!(f, Effect::Call { .. })),
        "no second read while one is in flight"
    );
    // The setting changes; the old read answers with the old value: no key,
    // a new read instead, whose answer decides.
    r.tv.always_ready = always_ready_reply("off");
    let fx = r.c.on_reply(
        Purpose::Read(Topic::AlwaysReady),
        Ok(always_ready_reply("alwaysReady")),
        ms(r.t0, 1100),
    );
    assert!(fx.iter().any(|f| matches!(
        f,
        Effect::Call {
            purpose: Purpose::Read(Topic::AlwaysReady),
            ..
        }
    )));
    r.run(fx);
    assert_eq!(r.set_names(), ["turnOffScreen"]);
    assert!(r.logged("Always Ready is off"));
}

#[test]
fn the_fallback_is_announced_only_when_the_screen_off_is_sent() {
    // Always Ready off, but the TV is in a transition: a refusal, and no
    // "screen off instead" line.
    let mut r = boot_normal(all_on(), Tv::new());
    r.power("Active", true, 100)
        .cmd(mp(MediaAction::TurnOff), 1000);
    assert!(r.sets.is_empty());
    assert!(!r.logged("instead of the power key"));
    assert!(
        r.logged("refused: the TV is changing power state"),
        "the real reason, not Always Ready"
    );
    // The screen control busy: refused too, not announced.
    let mut r = boot_normal(all_on(), Tv::new());
    r.cmd(sw_cmd(e::TV_SCREEN, false), 900)
        .cmd(mp(MediaAction::TurnOff), 1000);
    assert_eq!(r.set_names(), ["turnOffScreen"]);
    assert!(!r.logged("instead of the power key"));
}

#[test]
fn the_power_switch_turns_on_without_always_ready() {
    for word in ["off", "lifeOnScreen"] {
        let mut r = boot_normal(all_on(), tv_always_ready(word));
        assert_eq!(r.sw(e::TV_POWER), None, "the off conditions fail");
        r.power("ScreenOff", false, 1000)
            .cmd(sw_cmd(e::TV_POWER, true), 2000);
        assert_eq!(r.set_names(), ["turnOnScreen"], "{word}");
        let mut r = boot_normal(all_on(), tv_always_ready(word));
        r.power("ActiveStandby", false, 1000)
            .cmd(sw_cmd(e::TV_POWER, true), 1100)
            .tick(2600);
        assert_eq!(r.set_names(), ["powerOn"], "{word}");
    }
}

#[test]
fn media_player_features_that_are_off_are_refused() {
    let mut r = Rig::boot(
        Config {
            volume: false,
            mute: false,
            power_on: false,
            ..all_on()
        },
        Tv::new(),
    );
    r.cmd(mp(MediaAction::SetVolume(0.5)), 1000)
        .cmd(mp(MediaAction::VolumeUp), 1100)
        .cmd(mp(MediaAction::Mute(true)), 1200)
        .cmd(mp(MediaAction::TurnOn), 1300)
        .tick(3000);
    assert!(r.sets.is_empty(), "{:?}", r.set_names());
    assert!(r.logged("not enabled"));
    // Another id, or the media player switched off: ignored.
    let mut r = Rig::boot(
        Config {
            media_player: false,
            ..all_on()
        },
        Tv::new(),
    );
    assert!(r.state(e::TV_MEDIA_PLAYER).is_none(), "no state when off");
    r.cmd(mp(MediaAction::TurnOff), 1000);
    r.cmd(
        Command::Media {
            id: "tv_other".to_owned(),
            action: MediaAction::TurnOff,
        },
        1100,
    );
    assert!(r.sets.is_empty());
}

#[test]
fn media_player_logs_no_title_or_label() {
    let mut r = Rig::boot(all_on(), Tv::new());
    r.cmd(mp(MediaAction::SetVolume(0.5)), 1000)
        .tick(1300)
        .cmd(mp(MediaAction::TurnOff), 2000);
    for (_, m) in &r.logs {
        for w in ["App A", "App B", "HDMI 1", "app.a", "app.b"] {
            assert!(!m.contains(w), "{m}");
        }
    }
    assert!(r.logged("tv_media_player: volume set"));
}

#[test]
fn power_off_refusal_texts_are_readable() {
    for (r, text) in [
        (PowerOffRefusal::NotReady, "not known yet"),
        (PowerOffRefusal::AlwaysReadyOff, "suspend the TV"),
        (
            PowerOffRefusal::AlwaysReadyUnknown,
            "Always Ready is not known",
        ),
        (PowerOffRefusal::KillSwitch, "kill switch"),
        (PowerOffRefusal::WrongState, "Active or Screen Saver"),
        (PowerOffRefusal::Darkener("cooldown".into()), "cooldown"),
    ] {
        assert!(r.to_string().contains(text), "{r}");
    }
}

// ----- the two LG settings sensors (ADR 0008 decision 4, design 7) -----------

fn lg_reads(r: &Rig) -> Vec<Topic> {
    r.reads
        .iter()
        .map(|(t, _, _)| *t)
        .filter(|t| matches!(t, Topic::AlwaysReady | Topic::HandsFree))
        .collect()
}

#[test]
fn lg_sensors_are_read_only_diagnostic_text_sensors_with_fixed_ids() {
    let c = Controller::new(Config::default());
    let ents = c.entities();
    for (id, name) in [
        (e::LG_ALWAYS_READY, "Always Ready"),
        (e::LG_HANDS_FREE, "LG hands-free"),
    ] {
        let d = ents.iter().find(|d| d.id == id).unwrap();
        assert_eq!(
            (d.name, d.category, &d.kind),
            (name, Category::Diagnostic, &Kind::TextSensor)
        );
    }
    assert_eq!(e::LG_ALWAYS_READY, "lg_always_ready");
    assert_eq!(e::LG_HANDS_FREE, "lg_hands_free");
    // The switch is a config key: off removes both, and nothing is read.
    let mut c = Controller::new(Config {
        lg_settings: false,
        ..Config::default()
    });
    assert!(!c.entities().iter().any(|d| d.id.starts_with("lg_")));
    assert!(c.start().iter().all(|f| !matches!(
        f,
        Effect::Call {
            purpose: Purpose::Read(Topic::AlwaysReady | Topic::HandsFree),
            ..
        }
    )));
}

#[test]
fn lg_sensors_read_the_documented_calls_and_show_only_the_listed_values() {
    let mut r = Rig::boot(all_on(), Tv::new());
    let calls: Vec<_> = r
        .reads
        .iter()
        .filter(|(t, _, _)| matches!(t, Topic::AlwaysReady | Topic::HandsFree))
        .map(|(_, u, p)| (*u, p.as_str()))
        .collect();
    assert_eq!(
        calls,
        [
            (
                "luna://com.webos.settingsservice/getSystemSettings",
                r#"{"category":"general","keys":["lifeOnScreenMode"]}"#
            ),
            (
                "luna://com.webos.service.voiceinput/setting/getActivation",
                "{}"
            )
        ]
    );
    assert_eq!(r.text(e::LG_ALWAYS_READY).as_deref(), Some("Off"));
    assert_eq!(r.text(e::LG_HANDS_FREE).as_deref(), Some("Off"));
    // Every raw word maps to one of the four strings; an unknown word is
    // Other and the word stays in the log.
    for (word, label) in [
        ("none", "Off"),
        ("off", "Off"),
        ("alwaysReady", "On"),
        ("allEnabled", "On with wallpaper"),
        ("lifeOnScreen", "Other"),
        ("somethingNew", "Other"),
    ] {
        let mut tv = Tv::new();
        tv.always_ready = always_ready_reply(word);
        let r = Rig::boot(all_on(), tv);
        assert_eq!(r.text(e::LG_ALWAYS_READY).as_deref(), Some(label), "{word}");
    }
    let mut tv = Tv::new();
    tv.activation = activation_reply(true);
    let r2 = Rig::boot(all_on(), tv);
    assert_eq!(r2.text(e::LG_HANDS_FREE).as_deref(), Some("On"));
    // Nothing was written and no command exists for either sensor.
    r.cmd(sw_cmd(e::LG_ALWAYS_READY, true), 100)
        .cmd(btn(e::LG_HANDS_FREE), 200)
        .cmd(
            Command::Select {
                id: e::LG_ALWAYS_READY.to_owned(),
                option: "On".to_owned(),
            },
            300,
        );
    assert!(r.sets.is_empty() && r2.sets.is_empty());
}

#[test]
fn lg_sensors_poll_every_60_s_while_lit_or_dark_and_log_changes_made_elsewhere() {
    let mut r = Rig::boot(all_on(), Tv::new());
    let n0 = lg_reads(&r).len();
    assert_eq!(n0, 2, "the startup check");
    r.tick(59_000);
    assert_eq!(lg_reads(&r).len(), n0, "not before 60 s");
    // Changed at the TV (or by ctl.sh): seen by the next poll, logged once.
    r.tv.always_ready = always_ready_reply("alwaysReady");
    r.tv.activation = activation_reply(true);
    r.tick(60_100);
    assert_eq!(lg_reads(&r).len(), n0 + 2);
    assert_eq!(r.text(e::LG_ALWAYS_READY).as_deref(), Some("On"));
    assert_eq!(r.text(e::LG_HANDS_FREE).as_deref(), Some("On"));
    assert!(r.logs.iter().any(|(l, m)| *l == Level::Info
        && m == "lg setting: always-ready off -> on (not changed by the satellite)"));
    assert!(r.logs.iter().any(|(l, m)| *l == Level::Info
        && m == "lg setting: hands-free off -> on (not changed by the satellite)"));
    // The same value again: no line.
    let lines = r.logs.len();
    r.tick(120_200);
    assert_eq!(lg_reads(&r).len(), n0 + 4);
    assert_eq!(r.logs.len(), lines);
    // Screen Off is read too; standby and a transition are not, and the last
    // value stays shown in standby.
    r.power("ScreenOff", false, 121_000);
    r.tv.always_ready = always_ready_reply("allEnabled");
    r.tick(180_300);
    assert_eq!(
        r.text(e::LG_ALWAYS_READY).as_deref(),
        Some("On with wallpaper")
    );
    assert!(r.logs.iter().any(|(_, m)| m
        == "lg setting: always-ready on -> on with wallpaper (not changed by the satellite)"));
    let reads = lg_reads(&r).len();
    r.power("ActiveStandby", false, 181_000);
    r.tv.always_ready = always_ready_reply("off");
    r.tick(300_000);
    r.power("Active", true, 301_000);
    r.tick(420_000);
    assert_eq!(
        lg_reads(&r).len(),
        reads,
        "no read in standby or a transition"
    );
    assert_eq!(
        r.text(e::LG_ALWAYS_READY).as_deref(),
        Some("On with wallpaper")
    );
    // Lit again: the poll, long due, reads at the next tick.
    r.power("Active", false, 421_000);
    r.tick(421_100);
    assert_eq!(r.text(e::LG_ALWAYS_READY).as_deref(), Some("Off"));
    assert_eq!(lg_reads(&r).len(), reads + 2);
}

#[test]
fn lg_sensor_shows_other_for_an_unknown_word_and_keeps_the_word_to_the_log() {
    let mut r = Rig::boot(all_on(), Tv::new());
    r.tv.always_ready = always_ready_reply("lifeOnScreen");
    r.tick(60_100);
    assert_eq!(r.text(e::LG_ALWAYS_READY).as_deref(), Some("Other"));
    assert!(r.logged("always-ready off -> other (lifeOnScreen) (not changed by the satellite)"));
    // The state HA was sent never carries the raw word.
    let sent = format!("{:?}", r.c.sent_state(e::LG_ALWAYS_READY));
    assert!(!sent.contains("lifeOnScreen"), "{sent}");
}

#[test]
fn lg_sensor_without_an_answer_has_no_state_one_warn_and_recovers() {
    for bad in [
        r#"{"returnValue":false,"errorCode":-1,"errorText":"Denied"}"#,
        r#"{"returnValue":true}"#,
        "garbage",
        "",
    ] {
        let mut r = boot_with(all_on(), Topic::AlwaysReady, Ok(bad));
        assert_eq!(r.text(e::LG_ALWAYS_READY), None, "{bad}");
        assert_eq!(
            r.text(e::LG_HANDS_FREE).as_deref(),
            Some("Off"),
            "the other sensor is unaffected"
        );
        let warns = |r: &Rig| {
            r.logs
                .iter()
                .filter(|(l, m)| *l == Level::Warn && m.contains("cannot read the Always Ready"))
                .count()
        };
        assert_eq!(warns(&r), 1, "{bad}");
        // A poll that fails again is not logged again; no body in the log.
        r.tv.always_ready = bad.to_owned();
        r.tick(60_100);
        r.tick(120_200);
        assert_eq!(warns(&r), 1, "{bad}");
        // The TV answers again: the state returns without a change line.
        r.tv.always_ready = always_ready_reply("alwaysReady");
        r.tick(180_300);
        assert_eq!(r.text(e::LG_ALWAYS_READY).as_deref(), Some("On"));
        assert!(r.logged("the Always Ready setting answers again"));
        assert!(!r.logged("always-ready"));
    }
    // A timed-out startup read counts as failed.
    let mut c = Controller::new(all_on());
    let fx = c.start();
    assert!(!fx.is_empty());
    let fx = c.finish_probe(Instant::now());
    assert!(fx.iter().any(|f| matches!(f, Effect::Log(Level::Warn, m)
        if m.contains("cannot read the LG hands-free setting"))));
}

#[test]
fn lg_refresh_reads_both_at_once_only_where_the_poll_reads() {
    let mut r = Rig::boot(all_on(), Tv::new());
    let n0 = lg_reads(&r).len();
    // ctl.sh set changed Always Ready: HA shows it within the same second.
    r.tv.always_ready = always_ready_reply("alwaysReady");
    let fx = r.c.refresh_lg_settings(ms(r.t0, 2_000));
    assert_eq!(fx.len(), 2);
    r.run(fx);
    assert_eq!(lg_reads(&r).len(), n0 + 2);
    assert_eq!(r.text(e::LG_ALWAYS_READY).as_deref(), Some("On"));
    // The poll interval restarts from the refresh.
    r.tick(61_000);
    assert_eq!(lg_reads(&r).len(), n0 + 2);
    r.tick(62_100);
    assert_eq!(lg_reads(&r).len(), n0 + 4);
    // A read still in flight is followed by another one on the next tick.
    let fx = r.c.refresh_lg_settings(ms(r.t0, 70_000));
    assert_eq!(fx.len(), 2);
    let again = r.c.refresh_lg_settings(ms(r.t0, 70_100));
    assert!(again.is_empty(), "already running");
    r.run(fx);
    r.tick(70_300);
    assert_eq!(lg_reads(&r).len(), n0 + 8);
    // Not in standby, a transition or a controller that is off.
    r.power("ActiveStandby", false, 80_000);
    assert!(r.c.refresh_lg_settings(ms(r.t0, 81_000)).is_empty());
    r.power("Active", true, 82_000);
    assert!(r.c.refresh_lg_settings(ms(r.t0, 83_000)).is_empty());
    let mut off = Rig::boot(
        Config {
            lg_settings: false,
            ..all_on()
        },
        Tv::new(),
    );
    assert!(off.c.refresh_lg_settings(ms(off.t0, 1_000)).is_empty());
    assert!(lg_reads(&off).is_empty());
    off.tick(61_000);
    assert!(lg_reads(&off).is_empty());
    assert_eq!(off.state(e::LG_ALWAYS_READY), None);
    // Before the startup check nothing is read.
    let mut c = Controller::new(all_on());
    assert!(c.refresh_lg_settings(Instant::now()).is_empty());
}

#[test]
fn lg_sensors_are_read_again_after_a_resume() {
    let mut r = Rig::boot(all_on(), Tv::new());
    r.tv.activation = activation_reply(true);
    let fx = r.c.on_resume(ms(r.t0, 5_000));
    r.run(fx);
    assert_eq!(r.text(e::LG_HANDS_FREE).as_deref(), Some("On"));
    assert_eq!(lg_reads(&r).len(), 4);
}
