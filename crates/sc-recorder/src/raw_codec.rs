//! A tonic codec that passes message bodies through untouched.
//!
//! The recorder must store frames byte-for-byte as the game sent them, and
//! the replay server must send them back the same way. Decoding into typed
//! structs would drop unknown fields and change encoding, so we skip it.

use bytes::{Buf, BufMut, Bytes};
use tonic::Status;
use tonic::codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};

#[derive(Debug, Clone, Copy, Default)]
pub struct RawCodec;

impl Codec for RawCodec {
    type Encode = Bytes;
    type Decode = Bytes;
    type Encoder = RawCodec;
    type Decoder = RawCodec;

    fn encoder(&mut self) -> Self::Encoder {
        RawCodec
    }

    fn decoder(&mut self) -> Self::Decoder {
        RawCodec
    }
}

impl Encoder for RawCodec {
    type Item = Bytes;
    type Error = Status;

    fn encode(&mut self, item: Bytes, dst: &mut EncodeBuf<'_>) -> Result<(), Status> {
        dst.put(item);
        Ok(())
    }
}

impl Decoder for RawCodec {
    type Item = Bytes;
    type Error = Status;

    fn decode(&mut self, src: &mut DecodeBuf<'_>) -> Result<Option<Bytes>, Status> {
        // tonic hands us exactly one gRPC message, already de-framed.
        Ok(Some(src.copy_to_bytes(src.remaining())))
    }
}
