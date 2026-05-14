use rav1e::{Encoder, EncoderConfig, PixelLayout};
use bytes::BytesMut;

/// 创建一个 AV1 编码器
pub fn new_encoder(width: usize, height: usize) -> Encoder<u8> {
    let cfg = EncoderConfig {
        width,
        height,
        ..Default::default()
    };
    Encoder::new(&cfg).expect("Failed to create encoder")
}

/// 将 YUV420 原始帧压成 AV1 bitstream
pub async fn encode_frame(
    enc: &mut Encoder<u8>,
    yuv: &[u8],
) -> Result<BytesMut, rav1e::Error> {
    let mut frame = enc.new_frame();
    // 数据布局为 I420（YUV420）
    frame.copy_from_raw(yuv, PixelLayout::I420)?;
    
    enc.send_frame(frame)?;
    
    let mut data = BytesMut::new();
    while let Ok(pkt) = enc.receive_packet() {
        data.extend_from_slice(&pkt.data);
    }
    
    Ok(data)
}
