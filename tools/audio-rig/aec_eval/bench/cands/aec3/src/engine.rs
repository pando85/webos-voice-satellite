use aec3::nodes::audio::AudioFormat;
use aec3::pipelines::linear::{self, LinearPipeline};
pub const ENGINE_NAME: &str = "aec3-0.4.0(AEC3,pure-Rust)";
struct E(LinearPipeline, Vec<f32>);
impl Engine for E {
    fn process(&mut self, r: &[f32], m: &[f32], out: &mut Vec<f32>) {
        self.0.handle_render_frame(r).unwrap();
        let ok = self.0.process_capture_frame(m, &mut self.1).unwrap();
        if ok { out.extend_from_slice(&self.1); } else { out.extend(std::iter::repeat(0.0).take(FRAME)); }
    }
}
pub fn make(mode: &str) -> Box<dyn Engine> {
    let f = AudioFormat::ten_ms(SR, 1);
    let p = linear::builder(f, f)
        .enable_noise_suppression(mode.contains("ns"))
        .enable_gain_controller2(mode.contains("agc"))
        .enable_high_pass_filter(true)
        .build().unwrap();
    Box::new(E(p, vec![0.0; FRAME]))
}
