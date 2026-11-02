use aec_rs::{Aec, AecConfig};
pub const ENGINE_NAME: &str = "aec-rs-1.0.0(speexdsp MDF,C)";
struct E(Aec);
fn to_i16(x: &[f32]) -> Vec<i16> { x.iter().map(|v| (v * 32768.0).clamp(-32768.0, 32767.0) as i16).collect() }
impl Engine for E {
    fn process(&mut self, r: &[f32], m: &[f32], out: &mut Vec<f32>) {
        let (ri, mi) = (to_i16(r), to_i16(m));
        let mut o = vec![0i16; FRAME];
        self.0.cancel_echo(&mi, &ri, &mut o);
        out.extend(o.iter().map(|&v| v as f32 / 32768.0));
    }
}
pub fn make(mode: &str) -> Box<dyn Engine> {
    // 200 ms filter tail (3200 taps) at 16 kHz; preprocess = speexdsp residual echo suppress + NS
    let tail: i32 = std::env::var("SPEEX_TAIL").ok().and_then(|v| v.parse().ok()).unwrap_or(3200);
    let cfg = AecConfig { frame_size: FRAME, filter_length: tail, sample_rate: SR, enable_preprocess: mode.contains("ns") };
    Box::new(E(Aec::new(&cfg)))
}
