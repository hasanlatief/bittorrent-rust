use tokio::io::AsyncReadExt;

#[derive(Clone, Debug)]
pub enum Message {
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have(u32),
    Bitfield(BitfieldMsg),
    Request(RequestMsg),
    Piece(PieceMsg),
    Cancel,
}

impl Message {
    pub fn len(&self) -> u32 {
        1 + match self {
            Message::Choke | Message::Unchoke | Message::Interested | Message::NotInterested => 0,
            Message::Have(idx) => size_of_val(idx),
            Message::Bitfield(bitfield) => bitfield.bytes.len(),
            Message::Request(req) => req.len(),
            Message::Piece(piece) => piece.len(),
            Message::Cancel => todo!(),
        } as u32
    }
    pub fn id(&self) -> u8 {
        match self {
            Message::Choke => 0,
            Message::Unchoke => 1,
            Message::Interested => 2,
            Message::NotInterested => 3,
            Message::Have(_) => 4,
            Message::Bitfield(_) => 5,
            Message::Request(_) => 6,
            Message::Piece(_) => 7,
            Message::Cancel => 8,
        }
    }

    pub fn payload(self) -> Vec<u8> {
        match self {
            Message::Choke | Message::Unchoke | Message::Interested | Message::NotInterested => {
                Vec::new()
            }
            Message::Have(piece_idx) => piece_idx.to_be_bytes().to_vec(),
            Message::Bitfield(bitfield) => bitfield.bytes,
            Message::Request(req) => req.payload().to_vec(),
            Message::Piece(piece) => piece.payload(),
            Message::Cancel => todo!(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RequestMsg {
    piece_idx: u32,
    byte_idx: u32,
    length: u32,
}

impl RequestMsg {
    pub fn new(piece_idx: u32, byte_idx: u32, length: u32) -> Self {
        Self {
            piece_idx,
            byte_idx,
            length,
        }
    }

    pub async fn parse<R: tokio::io::AsyncBufRead + Unpin>(mut reader: R) -> anyhow::Result<Self> {
        Ok(Self {
            piece_idx: reader.read_u32().await?,
            byte_idx: reader.read_u32().await?,
            length: reader.read_u32().await?,
        })
    }

    fn payload(&self) -> [u8; 12] {
        *[
            self.piece_idx.to_be_bytes(),
            self.byte_idx.to_be_bytes(),
            self.length.to_be_bytes(),
        ]
        .as_flattened()
        .as_array()
        .unwrap()
    }

    pub fn len(&self) -> usize {
        size_of::<Self>()
    }
}

#[derive(Clone, Debug)]
pub struct PieceMsg {
    piece_idx: u32,
    byte_idx: u32,
    block: Vec<u8>,
}

impl PieceMsg {
    pub async fn parse<R: tokio::io::AsyncBufRead + Unpin>(mut reader: R) -> anyhow::Result<Self> {
        let piece_idx = reader.read_u32().await?;
        let byte_idx = reader.read_u32().await?;
        let mut block = Vec::new();
        reader.read_to_end(&mut block).await?;
        Ok(Self {
            piece_idx,
            byte_idx,
            block,
        })
    }
    fn payload(&self) -> Vec<u8> {
        let mut result = Vec::new();
        result.extend_from_slice(&self.piece_idx.to_be_bytes());
        result.extend_from_slice(&self.byte_idx.to_be_bytes());
        result.extend_from_slice(&self.block);
        result
    }

    pub fn piece_idx(&self) -> u32 {
        self.piece_idx
    }

    pub fn byte_idx(&self) -> u32 {
        self.byte_idx
    }

    pub fn block(&self) -> &[u8] {
        &self.block
    }

    pub fn len(&self) -> usize {
        size_of_val(&self.piece_idx) + size_of_val(&self.byte_idx) + self.block.len()
    }
}

#[derive(Debug, Clone)]
pub struct BitfieldMsg {
    bytes: Vec<u8>,
}

impl BitfieldMsg {
    pub async fn parse<R: tokio::io::AsyncBufRead + Unpin>(mut reader: R) -> anyhow::Result<Self> {
        let mut bits = Vec::new();
        reader.read_to_end(&mut bits).await?;
        Ok(Self { bytes: bits })
    }
    pub fn has_piece(&self, idx: u32) -> bool {
        self.bytes[(idx / 8) as usize] & (0x80 >> (idx % 8)) != 0
    }
}

#[test]
fn bitfield_has_piece() {
    let bitfield = BitfieldMsg {
        bytes: vec![0x81, 0x80, 0x04],
    };
    for byte in &bitfield.bytes {
        eprintln!("{byte:08b}");
    }
    assert!(bitfield.has_piece(0));
    assert!(bitfield.has_piece(7));
    assert!(bitfield.has_piece(8));
    assert!(bitfield.has_piece(21));
}
