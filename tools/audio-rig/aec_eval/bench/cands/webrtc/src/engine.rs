use webrtc_audio_processing::config::{EchoCanceller, GainController, GainController2, HighPassFilter, NoiseSuppression, NoiseSuppressionLevel};
use webrtc_audio_processing::{Config, Processor};
pub const ENGINE_NAME: &str = "webrtc-audio-processing-2.1.0(AEC3,C++)";
struct E(Processor);
impl Engine for E {
    fn process(&mut self, r: &[f32], m: &[f32], out: &mut Vec<f32>) {
        let mut rf = r.to_vec();
        self.0.process_render_frame([&mut rf[..]]).unwrap();
        let mut mf = m.to_vec();
        self.0.process_capture_frame([&mut mf[..]]).unwrap();
        out.extend_from_slice(&mf);
    }
}
pub fn make(mode: &str) -> Box<dyn Engine> {
    let p = Processor::new(SR).unwrap();
    let mut c = Config { echo_canceller: Some(EchoCanceller::Full { stream_delay_ms: None }), high_pass_filter: Some(HighPassFilter::default()), ..Default::default() };
    if mode.contains("ns") { c.noise_suppression = Some(NoiseSuppression { level: NoiseSuppressionLevel::Moderate, analyze_linear_aec_output: false }); }
    if mode.contains("agc") { c.gain_controller = Some(GainController::GainController2(GainController2::default())); }
    p.set_config(c);
    Box::new(E(p))
}
