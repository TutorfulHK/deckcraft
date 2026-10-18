//! MicroType Express (MTX) decompression, written from the W3C Member Submission
//! "MicroType Express (MTX) Font Format" (2008): the LZCOMP decoder (§3, Appendix C) and the
//! Compact Table Format (CTF) → TrueType rebuild (§4–§7).
//!
//! Every size read from the input is hostile: buffers are capped, reads are bounds-checked
//! and failures return [`MtxError`]. Optional tables that cannot be rebuilt (`hdmx`, `VDMX`)
//! are dropped rather than failing the whole font.

/// Largest decompressed block or rebuilt font we accept.
pub const MAX_OUT: usize = 16 * 1024 * 1024;
/// Largest glyph count we rebuild.
const MAX_GLYPHS: usize = 65_535;
/// Largest point count in one glyph.
const MAX_POINTS: usize = 65_535;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MtxError {
    /// The header or a block is malformed.
    Header,
    /// The compressed stream ended early or is inconsistent.
    Lzcomp,
    /// The CTF font is malformed.
    Ctf,
    /// A size exceeds [`MAX_OUT`].
    TooLarge,
}

// ---------------------------------------------------------------- bit input

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u16,
    left: u8,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Bits { data, pos: 0, buf: 0, left: 0 }
    }
    /// Most significant bit first (Appendix C `MTX_BITIO_input_bit`).
    fn bit(&mut self) -> Result<bool, MtxError> {
        if self.left == 0 {
            self.buf = u16::from(*self.data.get(self.pos).ok_or(MtxError::Lzcomp)?);
            self.pos += 1;
            self.left = 8;
        }
        self.left -= 1;
        self.buf <<= 1;
        Ok(self.buf & 0x100 != 0)
    }
    fn value(&mut self, n: u32) -> Result<u32, MtxError> {
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | u32::from(self.bit()?);
        }
        Ok(v)
    }
}

// ---------------------------------------------------------- adaptive Huffman

#[derive(Clone, Copy, Default)]
struct Node {
    up: usize,
    left: usize,
    right: usize,
    /// Symbol for a leaf, `None` for an internal node.
    code: Option<usize>,
    weight: u64,
}

/// Adaptive Huffman tree over symbols `0..range` (Appendix C `AHUFF`).
struct Ahuff {
    tree: Vec<Node>,
    index: Vec<usize>,
}

const ROOT: usize = 1;

impl Ahuff {
    fn new(range: usize) -> Result<Self, MtxError> {
        if !(2..=4096).contains(&range) {
            return Err(MtxError::Lzcomp);
        }
        let mut tree = vec![Node::default(); 2 * range];
        for i in 2..2 * range {
            if let Some(n) = tree.get_mut(i) {
                n.up = i / 2;
                n.weight = 1;
            }
        }
        for i in 1..range {
            if let Some(n) = tree.get_mut(i) {
                n.left = 2 * i;
                n.right = 2 * i + 1;
            }
        }
        let mut index = vec![0; range];
        for i in 0..range {
            if let Some(n) = tree.get_mut(range + i) {
                n.code = Some(i);
            }
            if let Some(x) = index.get_mut(i) {
                *x = range + i;
            }
        }
        // Internal weights are the sum of their children; index 0 is a sentinel heavier than all.
        for i in (1..range).rev() {
            let w = tree.get(2 * i).map_or(0, |n| n.weight) + tree.get(2 * i + 1).map_or(0, |n| n.weight);
            if let Some(n) = tree.get_mut(i) {
                n.weight = w;
            }
        }
        if let Some(n) = tree.get_mut(0) {
            n.weight = u64::MAX;
        }
        let mut h = Ahuff { tree, index };
        let bit_count2 = if range > 256 && range < 512 { bits_used(range - 256 - 1) + 1 } else { 0 };
        if bit_count2 != 0 {
            h.update_symbol(256)?;
            h.update_symbol(257)?;
            for _ in 0..12 {
                h.update_symbol(range - 3)?;
            }
            for _ in 0..6 {
                h.update_symbol(range - 2)?;
            }
        } else {
            for _ in 0..2 {
                for s in 0..range {
                    h.update_symbol(s)?;
                }
            }
        }
        Ok(h)
    }

    fn update_symbol(&mut self, s: usize) -> Result<(), MtxError> {
        let a = *self.index.get(s).ok_or(MtxError::Lzcomp)?;
        self.update(a)
    }

    fn w(&self, i: usize) -> Result<u64, MtxError> {
        self.tree.get(i).map(|n| n.weight).ok_or(MtxError::Lzcomp)
    }

    fn swap(&mut self, a: usize, b: usize) -> Result<(), MtxError> {
        let (upa, upb) = (self.tree.get(a).ok_or(MtxError::Lzcomp)?.up, self.tree.get(b).ok_or(MtxError::Lzcomp)?.up);
        self.tree.swap(a, b);
        for (i, up) in [(a, upa), (b, upb)] {
            let n = *self.tree.get(i).ok_or(MtxError::Lzcomp)?;
            if let Some(x) = self.tree.get_mut(i) {
                x.up = up;
            }
            match n.code {
                None => {
                    for c in [n.left, n.right] {
                        self.tree.get_mut(c).ok_or(MtxError::Lzcomp)?.up = i;
                    }
                }
                Some(code) => *self.index.get_mut(code).ok_or(MtxError::Lzcomp)? = i,
            }
        }
        Ok(())
    }

    fn update(&mut self, mut a: usize) -> Result<(), MtxError> {
        let mut guard = 0usize;
        while a != ROOT {
            guard += 1;
            if guard > self.tree.len() {
                return Err(MtxError::Lzcomp);
            }
            let wa = self.w(a)?;
            let mut b = a.checked_sub(1).ok_or(MtxError::Lzcomp)?;
            if self.w(b)? == wa {
                while b > 0 && self.w(b - 1)? == wa {
                    b -= 1;
                }
                if b > ROOT {
                    self.swap(a, b)?;
                    a = b;
                }
            }
            let n = self.tree.get_mut(a).ok_or(MtxError::Lzcomp)?;
            n.weight = wa.saturating_add(1);
            a = n.up;
        }
        let r = self.tree.get_mut(ROOT).ok_or(MtxError::Lzcomp)?;
        r.weight = r.weight.saturating_add(1);
        Ok(())
    }

    fn read(&mut self, bits: &mut Bits<'_>) -> Result<usize, MtxError> {
        let mut a = ROOT;
        for _ in 0..64 {
            let n = self.tree.get(a).ok_or(MtxError::Lzcomp)?;
            a = if bits.bit()? { n.right } else { n.left };
            if let Some(code) = self.tree.get(a).ok_or(MtxError::Lzcomp)?.code {
                self.update(a)?;
                return Ok(code);
            }
        }
        Err(MtxError::Lzcomp)
    }
}

fn bits_used(x: usize) -> u32 {
    (usize::BITS - x.leading_zeros()).max(1)
}

// ------------------------------------------------------------------ LZCOMP

const PRELOAD: usize = 2 * 32 * 96 + 4 * 256;

/// Decompress one LZCOMP block (§3.3, Appendix C `MTX_LZCOMP_UnPackMemory`, version ≥ 2).
pub fn lzcomp(data: &[u8]) -> Result<Vec<u8>, MtxError> {
    let mut bits = Bits::new(data);
    let run_length = bits.bit()?;
    let mut dist = Ahuff::new(8)?;
    let mut len = Ahuff::new(8)?;
    let out_len = usize::try_from(bits.value(24)?).map_err(|_| MtxError::TooLarge)?;
    if out_len > MAX_OUT {
        return Err(MtxError::TooLarge);
    }
    let mut ranges = 1u32;
    while (1usize << (3 * ranges)) < out_len {
        ranges += 1;
    }
    let dup2 = 256 + 8 * usize::try_from(ranges).map_err(|_| MtxError::Lzcomp)?;
    let (dup4, dup6) = (dup2 + 1, dup2 + 2);
    let mut sym = Ahuff::new(dup6 + 1)?;

    let mut buf = Vec::with_capacity(PRELOAD + out_len);
    for k in 0..32u8 {
        for j in 0..96u8 {
            buf.push(k);
            buf.push(j);
        }
    }
    for j in 0..=255u8 {
        buf.extend_from_slice(&[j; 4]);
    }
    let back =
        |buf: &Vec<u8>, d: usize| -> Result<u8, MtxError> { buf.len().checked_sub(d).and_then(|i| buf.get(i).copied()).ok_or(MtxError::Lzcomp) };
    while buf.len() - PRELOAD < out_len {
        let s = sym.read(&mut bits)?;
        if s < 256 {
            buf.push(u8::try_from(s).map_err(|_| MtxError::Lzcomp)?);
        } else if s == dup2 || s == dup4 || s == dup6 {
            let d = 2 * (s - dup2 + 1);
            let v = back(&buf, d)?;
            buf.push(v);
        } else {
            let first = s - 256;
            let n_dist = first / 8 + 1;
            let mut code = first % 8;
            let mut length = 0usize;
            let mut guard = 0;
            loop {
                length = (length << 2) | (code & 3);
                if code & 4 == 0 {
                    break;
                }
                guard += 1;
                if guard > 12 {
                    return Err(MtxError::Lzcomp);
                }
                code = len.read(&mut bits)?;
            }
            length += 2;
            let mut distance = 0usize;
            for _ in 0..n_dist {
                distance = (distance << 3) | dist.read(&mut bits)?;
            }
            distance += 1;
            if distance >= 512 {
                length += 1;
            }
            let start = buf.len().checked_sub(distance + length - 1).ok_or(MtxError::Lzcomp)?;
            if buf.len() - PRELOAD + length > out_len {
                return Err(MtxError::Lzcomp);
            }
            for j in 0..length {
                let v = *buf.get(start + j).ok_or(MtxError::Lzcomp)?;
                buf.push(v);
            }
        }
    }
    let raw = buf.get(PRELOAD..).ok_or(MtxError::Lzcomp)?;
    if !run_length {
        return Ok(raw.to_vec());
    }
    // Run-length layer (`MTX_RUNLENGTHCOMP_SaveBytes`): first byte is the escape; escape, 0
    // is a literal escape; escape, n, b is n copies of b.
    let mut out = Vec::with_capacity(raw.len());
    let mut it = raw.iter().copied();
    let esc = it.next().ok_or(MtxError::Lzcomp)?;
    while let Some(v) = it.next() {
        if v != esc {
            out.push(v);
            continue;
        }
        let n = it.next().ok_or(MtxError::Lzcomp)?;
        if n == 0 {
            out.push(esc);
        } else {
            let b = it.next().ok_or(MtxError::Lzcomp)?;
            if out.len() + usize::from(n) > MAX_OUT {
                return Err(MtxError::TooLarge);
            }
            out.extend(std::iter::repeat_n(b, usize::from(n)));
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- MTX file

fn u24(b: &[u8], at: usize) -> Option<usize> {
    let s = b.get(at..at + 3)?;
    Some((usize::from(*s.first()?) << 16) | (usize::from(*s.get(1)?) << 8) | usize::from(*s.get(2)?))
}

/// Decode an MTX font (§2) into a TrueType file starting with `0x00010000`.
pub fn decode(data: &[u8]) -> Result<Vec<u8>, MtxError> {
    if data.first() != Some(&3) {
        return Err(MtxError::Header);
    }
    let (o2, o3) = (u24(data, 4).ok_or(MtxError::Header)?, u24(data, 7).ok_or(MtxError::Header)?);
    if !(10 <= o2 && o2 <= o3 && o3 <= data.len()) {
        return Err(MtxError::Header);
    }
    let b1 = lzcomp(data.get(10..o2).ok_or(MtxError::Header)?)?;
    let b2 = lzcomp(data.get(o2..o3).ok_or(MtxError::Header)?)?;
    let b3 = lzcomp(data.get(o3..).ok_or(MtxError::Header)?)?;
    ctf_to_ttf(&b1, &b2, &b3)
}

// -------------------------------------------------------------- CTF → TTF

struct Rd<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Rd<'a> {
    fn u8(&mut self) -> Result<u8, MtxError> {
        let v = *self.b.get(self.p).ok_or(MtxError::Ctf)?;
        self.p += 1;
        Ok(v)
    }
    fn u16(&mut self) -> Result<u16, MtxError> {
        Ok(u16::from_be_bytes([self.u8()?, self.u8()?]))
    }
    fn i16(&mut self) -> Result<i16, MtxError> {
        Ok(i16::from_be_bytes([self.u8()?, self.u8()?]))
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], MtxError> {
        let s = self.b.get(self.p..self.p.checked_add(n).ok_or(MtxError::Ctf)?).ok_or(MtxError::Ctf)?;
        self.p += n;
        Ok(s)
    }
    /// 255USHORT (§6.1.1).
    fn u255(&mut self) -> Result<u16, MtxError> {
        Ok(match self.u8()? {
            253 => self.u16()?,
            255 => 253 + u16::from(self.u8()?),
            254 => 506 + u16::from(self.u8()?),
            c => u16::from(c),
        })
    }
    /// 255SHORT (§6.1.2).
    fn s255(&mut self) -> Result<i16, MtxError> {
        let mut code = self.u8()?;
        if code == 253 {
            return self.i16();
        }
        let mut sign = 1i32;
        if code == 250 {
            sign = -1;
            code = self.u8()?;
        }
        let v = match code {
            255 => 250 + i32::from(self.u8()?),
            254 => 500 + i32::from(self.u8()?),
            c => i32::from(c),
        };
        i16::try_from(v * sign).map_err(|_| MtxError::Ctf)
    }
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(at)?, *b.get(at + 1)?]))
}
fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes([*b.get(at)?, *b.get(at + 1)?, *b.get(at + 2)?, *b.get(at + 3)?]))
}

/// Magnitude-dependent bit reader (§5.3), least significant bit first.
struct Mag<'a> {
    b: &'a [u8],
    bit: usize,
}

impl Mag<'_> {
    fn bit(&mut self) -> Result<bool, MtxError> {
        let byte = *self.b.get(self.bit / 8).ok_or(MtxError::Ctf)?;
        let v = byte >> (self.bit % 8) & 1 == 1;
        self.bit += 1;
        Ok(v)
    }
    fn value(&mut self) -> Result<i32, MtxError> {
        let mut n = 0i32;
        while self.bit()? {
            n += 1;
            if n > 1 << 16 {
                return Err(MtxError::Ctf);
            }
        }
        if n > 0 && self.bit()? {
            n = -n;
        }
        Ok(n)
    }
    fn next_byte(&self) -> usize {
        self.bit.div_ceil(8)
    }
}

/// Triplet coordinate encoding (§5.11): (byte count, x bits, y bits, Δx, Δy, x negative, y negative).
fn triplet(index: u8) -> (usize, u32, u32, i32, i32, bool, bool) {
    let i = i32::from(index);
    match index {
        0..=9 => (2, 0, 8, 0, (i / 2) * 256, false, i % 2 == 0),
        10..=19 => (2, 8, 0, ((i - 10) / 2) * 256, 0, (i - 10) % 2 == 0, false),
        20..=83 => {
            let j = i - 20;
            let (dx, dy) = (1 + 16 * (j / 16), 1 + 16 * ((j % 16) / 4));
            (2, 4, 4, dx, dy, j % 2 == 0, (j / 2) % 2 == 0)
        }
        84..=119 => {
            let j = i - 84;
            let (dx, dy) = (1 + 256 * (j / 12), 1 + 256 * ((j % 12) / 4));
            (3, 8, 8, dx, dy, j % 2 == 0, (j / 2) % 2 == 0)
        }
        120..=123 => (4, 12, 12, 0, 0, i % 2 == 0, (i / 2) % 2 == 0),
        _ => (5, 16, 16, 0, 0, i % 2 == 0, (i / 2) % 2 == 0),
    }
}

/// TrueType push instructions for `vals` (§6.2.1: regenerate the initial push burst).
fn push_ops(vals: &[i16], out: &mut Vec<u8>) {
    let mut i = 0;
    while i < vals.len() {
        let is_byte = |v: i16| (0..=255).contains(&v);
        let byte = vals.get(i).is_some_and(|v| is_byte(*v));
        let mut j = i;
        while j < vals.len() && j - i < 255 && vals.get(j).is_some_and(|v| is_byte(*v) == byte) {
            j += 1;
        }
        let run = vals.get(i..j).unwrap_or(&[]);
        let n = run.len();
        let small = n <= 8;
        match (byte, small) {
            (true, true) => out.push(0xB0 + u8::try_from(n - 1).unwrap_or(0)),
            (true, false) => out.extend([0x40, u8::try_from(n).unwrap_or(255)]),
            (false, true) => out.push(0xB8 + u8::try_from(n - 1).unwrap_or(0)),
            (false, false) => out.extend([0x41, u8::try_from(n).unwrap_or(255)]),
        }
        for v in run {
            if byte {
                out.push(u8::try_from(*v).unwrap_or(0));
            } else {
                out.extend(v.to_be_bytes());
            }
        }
        i = j;
    }
}

/// Glyph instructions: `pushCount` values from block 2, then `codeSize` bytes from block 3.
fn instructions(g: &mut Rd<'_>, push: &mut Rd<'_>, code: &mut Rd<'_>) -> Result<Vec<u8>, MtxError> {
    let push_count = usize::from(g.u255()?);
    let code_size = usize::from(g.u255()?);
    let mut vals: Vec<i16> = Vec::with_capacity(push_count);
    while vals.len() < push_count {
        let c = *push.b.get(push.p).ok_or(MtxError::Ctf)?;
        if c == 251 || c == 252 {
            push.p += 1;
            let a = *vals.len().checked_sub(2).and_then(|i| vals.get(i)).ok_or(MtxError::Ctf)?;
            vals.push(a);
            vals.push(push.s255()?);
            vals.push(a);
            if c == 252 {
                vals.push(push.s255()?);
                vals.push(a);
            }
        } else {
            vals.push(push.s255()?);
        }
    }
    if vals.len() != push_count {
        return Err(MtxError::Ctf);
    }
    let mut out = Vec::new();
    push_ops(&vals, &mut out);
    out.extend_from_slice(code.take(code_size)?);
    if out.len() > 0xFFFF {
        return Err(MtxError::Ctf);
    }
    Ok(out)
}

fn put16(o: &mut Vec<u8>, v: u16) {
    o.extend(v.to_be_bytes());
}
fn puti16(o: &mut Vec<u8>, v: i16) {
    o.extend(v.to_be_bytes());
}

fn clamp16(v: i32) -> i16 {
    i16::try_from(v.clamp(i32::from(i16::MIN), i32::from(i16::MAX))).unwrap_or(0)
}

/// One glyph from the CTF `glyf` stream as TrueType glyph bytes (empty for an empty glyph).
fn glyph(g: &mut Rd<'_>, push: &mut Rd<'_>, code: &mut Rd<'_>) -> Result<Vec<u8>, MtxError> {
    let mut n = g.i16()?;
    let mut o = Vec::new();
    if n < 0 {
        // Composite (§5.10): bbox, components copied as-is, optional instructions.
        puti16(&mut o, -1);
        o.extend_from_slice(g.take(8)?);
        let mut flags;
        let mut count = 0;
        loop {
            count += 1;
            if count > 4096 {
                return Err(MtxError::Ctf);
            }
            flags = g.u16()?;
            put16(&mut o, flags);
            let mut len = 2 + if flags & 0x1 != 0 { 4 } else { 2 };
            len += if flags & 0x8 != 0 {
                2
            } else if flags & 0x40 != 0 {
                4
            } else if flags & 0x80 != 0 {
                8
            } else {
                0
            };
            o.extend_from_slice(g.take(len)?);
            if flags & 0x20 == 0 {
                break;
            }
        }
        if flags & 0x100 != 0 {
            let ins = instructions(g, push, code)?;
            put16(&mut o, u16::try_from(ins.len()).map_err(|_| MtxError::Ctf)?);
            o.extend(ins);
        }
        return Ok(o);
    }
    if n == 0 {
        return Ok(o);
    }
    let mut bbox = None;
    if n == 0x7FFF {
        n = g.i16()?;
        bbox = Some([g.i16()?, g.i16()?, g.i16()?, g.i16()?]);
        if n <= 0 {
            return Err(MtxError::Ctf);
        }
    }
    let contours = usize::try_from(n).map_err(|_| MtxError::Ctf)?;
    let mut ends = Vec::with_capacity(contours);
    let mut total = 0usize;
    for i in 0..contours {
        let v = usize::from(g.u255()?);
        total = if i == 0 { v + 1 } else { total + v };
        if total > MAX_POINTS {
            return Err(MtxError::Ctf);
        }
        ends.push(total.checked_sub(1).ok_or(MtxError::Ctf)?);
    }
    let flags = g.take(total)?.to_vec();
    let (mut x, mut y) = (0i32, 0i32);
    let mut pts = Vec::with_capacity(total);
    for f in &flags {
        let (count, xb, yb, dx0, dy0, xn, yn) = triplet(f & 0x7F);
        let mut data = 0u64;
        for b in g.take(count - 1)? {
            data = (data << 8) | u64::from(*b);
        }
        let nbits = u32::try_from((count - 1) * 8).unwrap_or(0);
        let mask = |b: u32| (1u64 << b) - 1;
        let mut dx = i32::try_from((data >> (nbits - xb)) & mask(xb)).unwrap_or(0);
        let mut dy = i32::try_from((data >> (nbits - xb - yb)) & mask(yb)).unwrap_or(0);
        dx += dx0;
        dy += dy0;
        if xn {
            dx = -dx;
        }
        if yn {
            dy = -dy;
        }
        x = x.saturating_add(dx);
        y = y.saturating_add(dy);
        pts.push((x, y, f & 0x80 == 0));
    }
    let ins = instructions(g, push, code)?;
    let bbox = bbox.unwrap_or_else(|| {
        let (mut a, mut b, mut c, mut d) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for (px, py, _) in &pts {
            a = a.min(*px);
            b = b.min(*py);
            c = c.max(*px);
            d = d.max(*py);
        }
        if pts.is_empty() { [0; 4] } else { [clamp16(a), clamp16(b), clamp16(c), clamp16(d)] }
    });
    puti16(&mut o, n);
    for v in bbox {
        puti16(&mut o, v);
    }
    for e in &ends {
        put16(&mut o, u16::try_from(*e).map_err(|_| MtxError::Ctf)?);
    }
    put16(&mut o, u16::try_from(ins.len()).map_err(|_| MtxError::Ctf)?);
    o.extend(ins);
    // Flags + coordinates, short vectors where they fit.
    let (mut fl, mut xs, mut ys) = (Vec::with_capacity(total), Vec::new(), Vec::new());
    let (mut px, mut py) = (0i32, 0i32);
    for (cx, cy, on) in &pts {
        let mut f = u8::from(*on);
        let (dx, dy) = (cx - px, cy - py);
        (px, py) = (*cx, *cy);
        for (d, short, same, out) in [(dx, 0x02u8, 0x10u8, &mut xs), (dy, 0x04, 0x20, &mut ys)] {
            if d == 0 {
                f |= same;
            } else if d.abs() < 256 {
                f |= short;
                if d > 0 {
                    f |= same;
                }
                out.push(u8::try_from(d.abs()).unwrap_or(0));
            } else {
                out.extend(clamp16(d).to_be_bytes());
            }
        }
        fl.push(f);
    }
    o.extend(fl);
    o.extend(xs);
    o.extend(ys);
    Ok(o)
}

struct Table {
    tag: [u8; 4],
    data: Vec<u8>,
}

/// Rebuild `cvt ` (§5.2).
fn cvt(b: &[u8]) -> Result<Vec<u8>, MtxError> {
    let mut r = Rd { b, p: 0 };
    let n = usize::from(r.u16()?);
    let mut out = Vec::with_capacity(n * 2);
    let mut last = 0i16;
    for _ in 0..n {
        let code = r.u8()?;
        let v: i16 = match code {
            0..=237 => i16::from(code),
            248..=255 => {
                let v = i32::from(r.u8()?) + 238 * i32::from(code - 247);
                i16::try_from(v).map_err(|_| MtxError::Ctf)?
            }
            239..=247 => {
                let v = i32::from(r.u8()?) + 238 * i32::from(code - 239);
                i16::try_from(-v).map_err(|_| MtxError::Ctf)?
            }
            _ => r.i16()?,
        };
        last = last.wrapping_add(v);
        puti16(&mut out, last);
    }
    Ok(out)
}

/// Rebuild `hdmx` (§5.4).
fn hdmx(b: &[u8], advances: &[u16], upem: u16) -> Result<Vec<u8>, MtxError> {
    let mut r = Rd { b, p: 0 };
    let version = r.u16()?;
    if version >= 0x8000 {
        let mut out = b.to_vec();
        out.splice(0..2, (0xFFFF - version).to_be_bytes());
        return Ok(out);
    }
    let num = usize::from(r.u16()?);
    let _size = r.i16()?;
    let _ = r.i16()?;
    let n_glyphs = advances.len();
    let rec = (2 + n_glyphs).div_ceil(4) * 4;
    if num.saturating_mul(rec) > MAX_OUT || upem == 0 {
        return Err(MtxError::Ctf);
    }
    let mut ppems = Vec::with_capacity(num);
    for _ in 0..num {
        ppems.push((r.u8()?, r.u8()?));
    }
    let mut mag = Mag { b: b.get(r.p..).ok_or(MtxError::Ctf)?, bit: 0 };
    let mut out = Vec::with_capacity(8 + num * rec);
    put16(&mut out, version);
    put16(&mut out, u16::try_from(num).map_err(|_| MtxError::Ctf)?);
    out.extend(u32::try_from(rec).map_err(|_| MtxError::Ctf)?.to_be_bytes());
    let upem = i64::from(upem);
    for (ppem, max_w) in ppems {
        let start = out.len();
        out.push(ppem);
        out.push(max_w);
        for aw in advances {
            let p = ((64 * i64::from(ppem) * i64::from(*aw) + upem / 2) / upem + 32) / 64;
            let w = p + i64::from(mag.value()?);
            out.push(u8::try_from(w.clamp(0, 255)).unwrap_or(0));
        }
        out.resize(start + rec, 0);
    }
    Ok(out)
}

/// Rebuild `VDMX` (§5.5).
fn vdmx(b: &[u8]) -> Result<Vec<u8>, MtxError> {
    let mut r = Rd { b, p: 0 };
    let version = r.u16()?;
    if version >= 0x8000 {
        let mut out = b.to_vec();
        out.splice(0..2, (0xFFFF - version).to_be_bytes());
        return Ok(out);
    }
    let num_recs = usize::from(r.u16()?);
    let num_ratios = usize::from(r.u16()?);
    let ratios = r.take(num_ratios * 4)?.to_vec();
    let mut offsets = Vec::with_capacity(num_ratios);
    for _ in 0..num_ratios {
        offsets.push(usize::from(r.u16()?));
    }
    let mut p = *offsets.first().ok_or(MtxError::Ctf)?;
    let mut groups: Vec<(usize, Vec<u8>)> = Vec::with_capacity(num_recs);
    for _ in 0..num_recs {
        let ctf_off = p;
        let mut g = Rd { b, p };
        let recs = usize::from(g.u16()?);
        let ymax_m = i32::from(g.i16()?);
        let ymin_m = i32::from(g.i16()?);
        let mut mag = Mag { b: b.get(g.p..).ok_or(MtxError::Ctf)?, bit: 0 };
        let mut entries = Vec::with_capacity(recs * 6);
        let (mut predicted, mut first, mut last) = (8i32, 0i32, 0i32);
        for j in 0..recs {
            let ppem = predicted + mag.value()?;
            let ymax = (ppem * ymax_m + 1024) / 2048 + mag.value()?;
            let ymin = -((ppem * ymin_m + 1024) / 2048) + mag.value()?;
            put16(&mut entries, u16::try_from(ppem.clamp(0, 0xFFFF)).unwrap_or(0));
            puti16(&mut entries, clamp16(ymax));
            puti16(&mut entries, clamp16(ymin));
            if j == 0 {
                first = ppem;
            }
            last = ppem;
            predicted = ppem + 1;
        }
        let mut out = Vec::with_capacity(4 + entries.len());
        put16(&mut out, u16::try_from(recs).map_err(|_| MtxError::Ctf)?);
        out.push(u8::try_from(first.clamp(0, 255)).unwrap_or(0));
        out.push(u8::try_from(last.clamp(0, 255)).unwrap_or(0));
        out.extend(entries);
        groups.push((ctf_off, out));
        p = g.p + mag.next_byte();
    }
    let header = 6 + num_ratios * 6;
    let mut out = Vec::new();
    put16(&mut out, version);
    put16(&mut out, u16::try_from(num_recs).map_err(|_| MtxError::Ctf)?);
    put16(&mut out, u16::try_from(num_ratios).map_err(|_| MtxError::Ctf)?);
    out.extend(ratios);
    let mut starts = Vec::with_capacity(groups.len());
    let mut at = header;
    for (_, g) in &groups {
        starts.push(at);
        at += g.len();
    }
    for off in offsets {
        let i = groups.iter().position(|(o, _)| *o == off).unwrap_or(0);
        put16(&mut out, u16::try_from(*starts.get(i).unwrap_or(&header)).map_err(|_| MtxError::Ctf)?);
    }
    for (_, g) in groups {
        out.extend(g);
    }
    Ok(out)
}

fn checksum(d: &[u8]) -> u32 {
    d.chunks(4).fold(0u32, |s, c| {
        let mut w = [0u8; 4];
        for (i, b) in c.iter().enumerate() {
            if let Some(x) = w.get_mut(i) {
                *x = *b;
            }
        }
        s.wrapping_add(u32::from_be_bytes(w))
    })
}

/// Rebuild a TrueType font from the three CTF blocks (§4–§7).
pub fn ctf_to_ttf(b1: &[u8], b2: &[u8], b3: &[u8]) -> Result<Vec<u8>, MtxError> {
    let num_tables = usize::from(be16(b1, 4).ok_or(MtxError::Ctf)?);
    if num_tables == 0 || num_tables > 256 {
        return Err(MtxError::Ctf);
    }
    let mut raw: Vec<([u8; 4], &[u8])> = Vec::with_capacity(num_tables);
    for i in 0..num_tables {
        let e = 12 + 16 * i;
        let tag = b1.get(e..e + 4).ok_or(MtxError::Ctf)?;
        let off = usize::try_from(be32(b1, e + 8).ok_or(MtxError::Ctf)?).map_err(|_| MtxError::Ctf)?;
        let len = usize::try_from(be32(b1, e + 12).ok_or(MtxError::Ctf)?).map_err(|_| MtxError::Ctf)?;
        let data = if len == 0 { &[][..] } else { b1.get(off..off.checked_add(len).ok_or(MtxError::Ctf)?).ok_or(MtxError::Ctf)? };
        raw.push(([*tag.first().unwrap_or(&0), *tag.get(1).unwrap_or(&0), *tag.get(2).unwrap_or(&0), *tag.get(3).unwrap_or(&0)], data));
    }
    let find = |t: &[u8; 4]| raw.iter().find(|(tag, _)| tag == t).map(|(_, d)| *d);
    let maxp = find(b"maxp").ok_or(MtxError::Ctf)?;
    let head = find(b"head").ok_or(MtxError::Ctf)?;
    let n_glyphs = usize::from(be16(maxp, 4).ok_or(MtxError::Ctf)?).min(MAX_GLYPHS);
    let upem = be16(head, 18).ok_or(MtxError::Ctf)?;
    let short_loca = be16(head, 50).ok_or(MtxError::Ctf)? == 0;

    // glyf + loca.
    let glyf_ctf = find(b"glyf").ok_or(MtxError::Ctf)?;
    let (mut g, mut push, mut code) = (Rd { b: glyf_ctf, p: 0 }, Rd { b: b2, p: 0 }, Rd { b: b3, p: 0 });
    let mut glyf = Vec::new();
    let mut loca = Vec::with_capacity(n_glyphs + 1);
    for _ in 0..n_glyphs {
        loca.push(glyf.len());
        let gl = glyph(&mut g, &mut push, &mut code)?;
        glyf.extend(gl);
        while glyf.len() % 4 != 0 {
            glyf.push(0);
        }
        if glyf.len() > MAX_OUT {
            return Err(MtxError::TooLarge);
        }
    }
    loca.push(glyf.len());
    if push.p != b2.len() || code.p != b3.len() {
        log::warn!("MTX: push/code streams not fully consumed ({}/{}, {}/{})", push.p, b2.len(), code.p, b3.len());
    }
    let use_short = short_loca && glyf.len() <= 0x1FFFE;
    let mut loca_bytes = Vec::new();
    for o in &loca {
        if use_short {
            put16(&mut loca_bytes, u16::try_from(o / 2).map_err(|_| MtxError::Ctf)?);
        } else {
            loca_bytes.extend(u32::try_from(*o).map_err(|_| MtxError::Ctf)?.to_be_bytes());
        }
    }

    // Advance widths for hdmx.
    let hhea = find(b"hhea");
    let hmtx = find(b"hmtx").unwrap_or(&[]);
    let n_h = hhea.and_then(|h| be16(h, 34)).map_or(0, usize::from);
    let advances: Vec<u16> = (0..n_glyphs).map(|i| be16(hmtx, 4 * i.min(n_h.saturating_sub(1))).unwrap_or(0)).collect();

    let mut tables = Vec::with_capacity(raw.len());
    for (tag, data) in &raw {
        let data = match tag {
            b"glyf" => glyf.clone(),
            b"loca" => loca_bytes.clone(),
            b"cvt " => cvt(data)?,
            b"hdmx" => match hdmx(data, &advances, upem) {
                Ok(d) => d,
                Err(_) => continue,
            },
            b"VDMX" => match vdmx(data) {
                Ok(d) => d,
                Err(_) => continue,
            },
            b"head" => {
                let mut h = data.to_vec();
                if let Some(s) = h.get_mut(8..12) {
                    s.copy_from_slice(&[0; 4]);
                }
                if let Some(s) = h.get_mut(50..52) {
                    s.copy_from_slice(&[0, u8::from(!use_short)]);
                }
                h
            }
            _ => data.to_vec(),
        };
        tables.push(Table { tag: *tag, data });
    }
    tables.sort_by_key(|a| a.tag);

    let n = u16::try_from(tables.len()).map_err(|_| MtxError::Ctf)?;
    let es = 15 - n.leading_zeros().min(15) as u16;
    let sr = (1u16 << es) * 16;
    let mut out = Vec::new();
    out.extend(0x0001_0000u32.to_be_bytes());
    put16(&mut out, n);
    put16(&mut out, sr);
    put16(&mut out, es);
    put16(&mut out, (n * 16).saturating_sub(sr));
    let mut off = 12 + 16 * tables.len();
    let mut head_at = None;
    for t in &tables {
        out.extend(t.tag);
        out.extend(checksum(&t.data).to_be_bytes());
        out.extend(u32::try_from(off).map_err(|_| MtxError::TooLarge)?.to_be_bytes());
        out.extend(u32::try_from(t.data.len()).map_err(|_| MtxError::TooLarge)?.to_be_bytes());
        off += t.data.len().div_ceil(4) * 4;
    }
    for t in &tables {
        if &t.tag == b"head" {
            head_at = Some(out.len());
        }
        out.extend(&t.data);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    if out.len() > MAX_OUT {
        return Err(MtxError::TooLarge);
    }
    if let Some(h) = head_at {
        let adj = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
        if let Some(s) = out.get_mut(h + 8..h + 12) {
            s.copy_from_slice(&adj.to_be_bytes());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test-only bit writer + adaptive Huffman encoder (the inverse of `Ahuff::read`).
    #[derive(Default)]
    struct W {
        bytes: Vec<u8>,
        cur: u8,
        n: u8,
    }
    impl W {
        fn bit(&mut self, b: bool) {
            self.cur = (self.cur << 1) | u8::from(b);
            self.n += 1;
            if self.n == 8 {
                self.bytes.push(self.cur);
                (self.cur, self.n) = (0, 0);
            }
        }
        fn value(&mut self, v: u32, n: u32) {
            for i in (0..n).rev() {
                self.bit(v >> i & 1 == 1);
            }
        }
        fn finish(mut self) -> Vec<u8> {
            while self.n != 0 {
                self.bit(false);
            }
            self.bytes
        }
    }
    fn put(h: &mut Ahuff, w: &mut W, s: usize) {
        let leaf = h.index[s];
        let (mut a, mut path) = (leaf, vec![]);
        while a != ROOT {
            let up = h.tree[a].up;
            path.push(h.tree[up].right == a);
            a = up;
        }
        for b in path.into_iter().rev() {
            w.bit(b);
        }
        h.update(leaf).unwrap();
    }

    enum Item {
        Lit(u8),
        Dup(usize),
        /// (length, distance) with distance < 512 and length ≥ 2.
        Copy(usize, usize),
    }

    /// Encode `items` producing `out_len` bytes as an LZCOMP block (no run-length layer).
    fn lz(items: &[Item], out_len: usize) -> Vec<u8> {
        let mut w = W::default();
        w.bit(false);
        let (mut dist, mut len) = (Ahuff::new(8).unwrap(), Ahuff::new(8).unwrap());
        w.value(u32::try_from(out_len).unwrap(), 24);
        let mut ranges = 1;
        while (1usize << (3 * ranges)) < out_len {
            ranges += 1;
        }
        let dup2 = 256 + 8 * ranges;
        let mut sym = Ahuff::new(dup2 + 3).unwrap();
        for it in items {
            match it {
                Item::Lit(b) => put(&mut sym, &mut w, usize::from(*b)),
                Item::Dup(d) => put(&mut sym, &mut w, dup2 + d / 2 - 1),
                Item::Copy(l, d) => {
                    // length - 2 in base-4 digits, most significant first, stop bit on the last.
                    let mut digits = vec![];
                    let mut v = l - 2;
                    loop {
                        digits.push(v & 3);
                        v >>= 2;
                        if v == 0 {
                            break;
                        }
                    }
                    digits.reverse();
                    let mut dd = vec![];
                    let mut v = d - 1;
                    loop {
                        dd.push(v & 7);
                        v >>= 3;
                        if v == 0 {
                            break;
                        }
                    }
                    dd.reverse();
                    let last = digits.len() - 1;
                    let code = |i: usize, x: usize| if i == last { x } else { x | 4 };
                    put(&mut sym, &mut w, 256 + (dd.len() - 1) * 8 + code(0, digits[0]));
                    for (i, x) in digits.iter().enumerate().skip(1) {
                        put(&mut len, &mut w, code(i, *x));
                    }
                    for x in dd {
                        put(&mut dist, &mut w, x);
                    }
                }
            }
        }
        w.finish()
    }

    fn lits(b: &[u8]) -> Vec<u8> {
        lz(&b.iter().map(|x| Item::Lit(*x)).collect::<Vec<_>>(), b.len())
    }

    #[test]
    fn lzcomp_literals_dups_and_copies() {
        let data: Vec<u8> = (0..300u32).map(|i| (i * 37 % 251) as u8).collect();
        assert_eq!(lzcomp(&lits(&data)).unwrap(), data);
        // a b c, DUP2 → b, then copies: the copy starts `distance + length - 1` bytes back
        // (§3.3), so (2, 1) repeats the last two bytes and (4, 2) the four before the last one.
        let items = [Item::Lit(b'a'), Item::Lit(b'b'), Item::Lit(b'c'), Item::Dup(2), Item::Copy(2, 1), Item::Copy(4, 2), Item::Lit(b'z')];
        assert_eq!(lzcomp(&lz(&items, 11)).unwrap(), b"abcbcbbcbcz");
        // A copy reaching back into the preloaded dictionary at the very start: the last
        // preload bytes are 255 ×4.
        assert_eq!(lzcomp(&lz(&[Item::Copy(3, 1)], 3)).unwrap(), [255, 255, 255]);
        // Run-length layer: escape 0xEE; EE 00 → EE, EE 04 41 → AAAA.
        let mut s = lits(&[0xEE, 1, 0xEE, 0, 0xEE, 4, 0x41, 2]);
        s[0] |= 0x80;
        // Setting the first bit changes nothing else: the run-length flag is that bit alone.
        assert_eq!(lzcomp(&s).unwrap(), [1, 0xEE, 0x41, 0x41, 0x41, 0x41, 2]);
    }

    /// A one-glyph CTF font (a triangle with hinting) and its blocks.
    fn ctf() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let mut head = vec![0u8; 54];
        head[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        head[12..16].copy_from_slice(&0x5F0F_3CF5u32.to_be_bytes());
        head[18..20].copy_from_slice(&1000u16.to_be_bytes());
        let mut maxp = vec![0u8; 32];
        maxp[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        maxp[4..6].copy_from_slice(&2u16.to_be_bytes());
        let mut hhea = vec![0u8; 36];
        hhea[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        hhea[34..36].copy_from_slice(&1u16.to_be_bytes());
        let hmtx = vec![0x01, 0xF4, 0, 0, 0, 0];
        // Glyph 0 empty; glyph 1: one contour of 3 points (end point 2), flags on/on/off,
        // triplets: (0,+100) idx1 b=100; (+50,0) idx11 b=50; (-50,-100)… use 3-byte idx84+? keep simple:
        // idx 0 (dy = -b): b=100 → (0,-100).
        let glyf = vec![
            0, 0, // glyph 0: empty
            0, 1, // glyph 1: 1 contour
            2, // end point of first contour
            1, 11, 0x80, // flags: idx1, idx11, idx0 off-curve
            100, 50, 100, // coordinate bytes
            3, 1, // pushCount 3, codeSize 1
        ];
        // cvt: 2 entries, +10 then -3 → 10, 7.
        let cvt = vec![0, 2, 10, 239, 3];
        let tables: [(&[u8; 4], Vec<u8>); 7] =
            [(b"cvt ", cvt), (b"glyf", glyf), (b"head", head), (b"hhea", hhea), (b"hmtx", hmtx), (b"loca", vec![]), (b"maxp", maxp)];
        let mut b1 = vec![0, 1, 0, 0, 0, 7, 0, 64, 0, 2, 0, 48];
        let mut off = 12 + 16 * tables.len();
        let mut body: Vec<u8> = vec![];
        for (tag, d) in &tables {
            b1.extend(*tag);
            b1.extend([0; 4]);
            let o = if d.is_empty() { 0 } else { off };
            b1.extend(u32::try_from(o).unwrap().to_be_bytes());
            b1.extend(u32::try_from(d.len()).unwrap().to_be_bytes());
            body.extend(d);
            off += d.len();
        }
        b1.extend(body);
        // Push: 7, 300 (255-code: 255 50), Hop… keep it plain; 3 values.
        let b2 = vec![7, 255, 50, 250, 4];
        let b3 = vec![0x2E]; // MDAP[1]
        (b1, b2, b3)
    }

    fn mtx_of(b1: &[u8], b2: &[u8], b3: &[u8]) -> Vec<u8> {
        let (c1, c2, c3) = (lits(b1), lits(b2), lits(b3));
        let o2 = 10 + c1.len();
        let o3 = o2 + c2.len();
        let mut f = vec![3, 0, 0, 0];
        f.extend(&u32::try_from(o2).unwrap().to_be_bytes()[1..]);
        f.extend(&u32::try_from(o3).unwrap().to_be_bytes()[1..]);
        f.extend(c1);
        f.extend(c2);
        f.extend(c3);
        f
    }

    fn table<'a>(f: &'a [u8], tag: &[u8; 4]) -> &'a [u8] {
        let n = usize::from(be16(f, 4).unwrap());
        for i in 0..n {
            let e = 12 + 16 * i;
            if &f[e..e + 4] == tag {
                let o = be32(f, e + 8).unwrap() as usize;
                let l = be32(f, e + 12).unwrap() as usize;
                return &f[o..o + l];
            }
        }
        panic!("no table");
    }

    #[test]
    fn ctf_rebuilds_glyf_loca_cvt() {
        let (b1, b2, b3) = ctf();
        let ttf = decode(&mtx_of(&b1, &b2, &b3)).unwrap();
        assert_eq!(&ttf[..4], &[0, 1, 0, 0]);
        assert_eq!(checksum(&ttf), 0xB1B0_AFBA, "checkSumAdjustment");
        assert_eq!(table(&ttf, b"cvt "), [0, 10, 0, 7]);
        let loca = table(&ttf, b"loca");
        assert_eq!(&loca[..4], &[0, 0, 0, 0], "empty glyph 0 (short loca)");
        let glyf = table(&ttf, b"glyf");
        let g = &glyf[usize::from(be16(loca, 2).unwrap()) * 2..];
        // numContours 1, bbox (0,0)-(50,100) computed from points (0,100),(50,100),(50,0).
        assert_eq!(&g[..10], &[0, 1, 0, 0, 0, 0, 0, 50, 0, 100]);
        assert_eq!(&g[10..12], &[0, 2], "end point");
        // Instructions: PUSHW[2]? No — 7 and 300 and -4: 7 is a byte, 300/-4 words.
        let ilen = usize::from(be16(g, 12).unwrap());
        assert_eq!(&g[14..14 + ilen], &[0xB0, 7, 0xB9, 0x01, 0x2C, 0xFF, 0xFC, 0x2E]);
    }

    #[test]
    fn ctf_hop_codes() {
        let (b1, _, b3) = ctf();
        // pushCount 3 in glyf → A, X1, Hop3(X2) gives 5 values: patch pushCount to 5.
        let mut b1 = b1;
        let at = b1.windows(2).rposition(|w| w == [3, 1]).unwrap();
        b1[at] = 5;
        let b2 = vec![9, 1, 251, 2];
        let ttf = decode(&mtx_of(&b1, &b2, &b3)).unwrap();
        let loca = table(&ttf, b"loca");
        let g = &table(&ttf, b"glyf")[usize::from(be16(loca, 2).unwrap()) * 2..];
        let ilen = usize::from(be16(g, 12).unwrap());
        assert_eq!(&g[14..14 + ilen], &[0xB4, 9, 1, 9, 2, 9, 0x2E]);
    }

    #[test]
    fn magnitude_example_from_spec() {
        let mut m = Mag { b: &[0x90, 0x48, 0x84], bit: 0 };
        let got: Vec<i32> = (0..13).map(|_| m.value().unwrap()).collect();
        assert_eq!(got, [0, 0, 0, 0, 1, 1, 0, 1, 1, 0, 1, 0, 0]);
        assert!(m.value().is_err(), "a value running off the end is an error");
        let mut m = Mag { b: &[0b0001_1011], bit: 0 };
        assert_eq!((m.value().unwrap(), m.value().unwrap()), (-2, 1));
    }

    #[test]
    fn hostile_input_never_panics() {
        let (b1, b2, b3) = ctf();
        let good = mtx_of(&b1, &b2, &b3);
        let mut seed = 0x1234_5678u64;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for i in 0..3000 {
            let mut d = good.clone();
            match i % 4 {
                0 => {
                    let n = (rnd() % 8) as usize + 1;
                    for _ in 0..n {
                        let at = (rnd() as usize) % d.len();
                        d[at] = rnd() as u8;
                    }
                }
                1 => d.truncate((rnd() as usize) % d.len()),
                2 => d = (0..(rnd() % 400) as usize).map(|_| rnd() as u8).collect(),
                _ => {
                    let at = (rnd() as usize) % d.len();
                    d.insert(at, rnd() as u8);
                }
            }
            if !d.is_empty() && i % 3 == 0 {
                d[0] = 3;
            }
            let _ = decode(&d);
            let _ = lzcomp(&d);
            let _ = ctf_to_ttf(&d, &d, &d);
        }
        // The largest declared size (24 bits, under the 16 MB cap) with no data ends cleanly.
        let mut w = W::default();
        w.bit(false);
        w.value(0xFF_FFFF, 24);
        assert_eq!(lzcomp(&w.finish()), Err(MtxError::Lzcomp));
    }
}
