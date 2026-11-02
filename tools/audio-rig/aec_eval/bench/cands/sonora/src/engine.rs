use sonora::config::{EchoCanceller, GainController2, HighPassFilter, NoiseSuppression};
use sonora::{AudioProcessing, Config, StreamConfig};
pub const ENGINE_NAME: &str = "sonora-0.2.0(AEC3,pure-Rust)";
struct E(AudioProcessing, Vec<f32>, Vec<f32>);
impl Engine for E {
    fn process(&mut self, r: &[f32], m: &[f32], out: &mut Vec<f32>) {
        self.0.process_render_f32(&[r], &mut [&mut self.1[..]]).unwrap();
        self.0.process_capture_f32(&[m], &mut [&mut self.2[..]]).unwrap();
        out.extend_from_slice(&self.2);
    }
}
pub fn make(mode: &str) -> Box<dyn Engine> {
    let mut c = Config { echo_canceller: Some(EchoCanceller::default()), high_pass_filter: Some(HighPassFilter::default()), ..Default::default() };
    if mode.contains("ns") { c.noise_suppression = Some(NoiseSuppression::default()); }
    if mode.contains("agc") { c.gain_controller2 = Some(GainController2::default()); }
    let sc = StreamConfig::new(SR, 1);
    let apm = AudioProcessing::builder().config(c).capture_config(sc).render_config(sc).build();
    Box::new(E(apm, vec![0.0; FRAME], vec![0.0; FRAME]))
}
