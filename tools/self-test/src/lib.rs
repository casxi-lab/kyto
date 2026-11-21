//! The bench self-test kernel for the Raspberry Pi 5 (BCM2712, AArch64),
//! written as the instruction words it is made of, so that its bytes follow
//! from this source alone. The README of this directory says what it does
//! and the state of the board that it relies on, with their sources.

/// The physical address of UART10, the PL011 UART of the debug connector.
pub const UART: u64 = 0x10_7d00_1000;

/// The line that the kernel writes once, when it starts.
pub const READY: &str = "kyto self-test: ready";

/// The start of every answer, which no other line of the kernel has.
pub const ECHO: &str = "kyto self-test: echo ";

// PL011 register offsets in words; the flags register's TXFF and RXFE bits;
// and the control register's UARTEN, TXE and RXE bits.
const DR: u32 = 0;
const FR: u32 = 0x18 / 4;
const CR: u32 = 0x30 / 4;
const TXFF: u32 = 5;
const RXFE: u32 = 4;
const ENABLE: u32 = 0x301;

// Registers kept across calls: the UART base, the bytes of the line, its
// first byte, and the return address of a routine that calls another. X30
// is the link register and 31 the zero register.
const BASE: u32 = 19;
const COUNT: u32 = 20;
const FIRST: u32 = 21;
const LINK: u32 = 22;
const LR: u32 = 30;
const ZR: u32 = 31;

/// The bytes of the kernel image, `kernel_2712.img`.
pub fn kernel() -> Vec<u8> {
    let mut a = Asm::default();
    // The 64-byte header of an arm64 Linux image: a branch over the header,
    // a load offset of 0, the image size, written last, flags 0 (a
    // little-endian image), the magic number "ARM\x64" and no PE header.
    a.to(B, 0, "start");
    a.word(NOP);
    a.bytes.extend_from_slice(&[0; 48]);
    a.word(0x644d_5241);
    a.word(0);

    // Only the core whose affinity fields are all 0 goes on; any other core
    // waits for an event, for ever.
    a.label("start");
    a.word(MRS_X0_MPIDR_EL1);
    a.word(rri(AND, 0, 0, 24 - 1));
    a.to(CBNZ, 0, "park");
    // Set UARTEN, TXE and RXE and keep the other bits and the rate and line
    // settings that the firmware left; then discard what was received.
    a.load(BASE, UART);
    a.word(rri(LDR_W, 0, BASE, CR));
    a.word(movz(1, ENABLE, 0));
    a.word(rrr(ORR, 0, 0, 1));
    a.word(rri(STR_W, 0, BASE, CR));
    a.label("drain");
    a.word(rri(LDR_W, 0, BASE, FR));
    a.to(TBNZ, RXFE << 19, "ready");
    a.word(rri(LDR_W, 0, BASE, DR));
    a.to(B, 0, "drain");
    a.label("ready");
    a.to(ADR, 1, "ready_text");
    a.to(BL, 0, "puts");

    // Each byte of a line is written as it arrives, after the start of the
    // answer before its first byte; CR or LF ends the line.
    a.label("line");
    a.word(movz(COUNT, 0, 0));
    a.label("next");
    a.to(BL, 0, "getc");
    for end in *b"\r\n" {
        a.word(rri(SUBS_W, ZR, 0, u32::from(end)));
        a.to(B_EQ, 0, "end");
    }
    a.to(CBNZ, COUNT, "byte");
    a.word(mov(FIRST, 0));
    a.to(ADR, 1, "echo_text");
    a.to(BL, 0, "puts");
    a.word(mov(0, FIRST));
    a.label("byte");
    a.to(BL, 0, "putc");
    a.word(rri(ADD, COUNT, COUNT, 1));
    a.to(B, 0, "next");
    a.label("end");
    a.to(CBZ, COUNT, "next");
    a.to(ADR, 1, "open_text");
    a.to(BL, 0, "puts");
    a.to(BL, 0, "putdec");
    a.to(ADR, 1, "close_text");
    a.to(BL, 0, "puts");
    a.to(B, 0, "line");

    a.label("park");
    a.word(WFE);
    a.to(B, 0, "park");

    // getc: waits for a received byte and returns it in X0.
    a.label("getc");
    a.word(rri(LDR_W, 0, BASE, FR));
    a.to(TBNZ, RXFE << 19, "getc");
    a.word(rri(LDR_W, 0, BASE, DR));
    a.word(rri(AND, 0, 0, 8 - 1));
    a.word(rri(RET, 0, LR, 0));
    // putc: waits for room in the transmit FIFO and writes the byte in W0.
    a.label("putc");
    a.word(rri(LDR_W, 2, BASE, FR));
    a.to(TBNZ, TXFF << 19 | 2, "putc");
    a.word(rri(STR_W, 0, BASE, DR));
    a.word(rri(RET, 0, LR, 0));
    // puts: writes the bytes from X1 up to the first 0.
    a.label("puts");
    a.word(mov(LINK, LR));
    a.label("puts_next");
    a.word(rri(LDRB_POST_1, 0, 1, 0));
    a.to(CBZ, 0, "puts_end");
    a.to(BL, 0, "putc");
    a.to(B, 0, "puts_next");
    a.label("puts_end");
    a.word(rri(RET, 0, LINK, 0));
    // putdec: writes COUNT, at least 1, in decimal: X4 runs through the
    // powers of ten from 10^19 down, X5 holds what is left to write and X7
    // is not 0 once a digit that is not 0 has been written.
    a.label("putdec");
    a.word(mov(LINK, LR));
    a.load(4, 10_000_000_000_000_000_000);
    a.word(mov(5, COUNT));
    a.word(movz(6, 10, 0));
    a.word(movz(7, 0, 0));
    a.label("digit");
    a.word(rrr(UDIV, 8, 5, 4));
    a.word(rrr(MSUB, 5, 8, 4) | 5 << 10);
    a.word(rrr(ORR, 7, 7, 8));
    a.to(CBZ, 7, "lower");
    a.word(rri(ADD, 0, 8, u32::from(b'0')));
    a.to(BL, 0, "putc");
    a.label("lower");
    a.word(rrr(UDIV, 4, 4, 6));
    a.to(CBNZ, 4, "digit");
    a.word(rri(RET, 0, LINK, 0));

    a.text("ready_text", &format!("\r\n{READY}\r\n"));
    a.text("echo_text", ECHO);
    a.text("open_text", " (");
    a.text("close_text", " bytes)\r\n");
    let mut image = a.finish();
    let size = image.len() as u64;
    image[16..24].copy_from_slice(&size.to_le_bytes());
    image
}

// Instruction words without their operands, which `rrr` (three registers)
// and `rri` (two registers and an immediate at bit 10) complete: AND's
// immediate is the number of low ones of its mask less 1, that of LDR and
// STR the offset in words; MSUB's fourth register is at bit 10.
const NOP: u32 = 0xd503_201f;
const WFE: u32 = 0xd503_205f;
const MRS_X0_MPIDR_EL1: u32 = 0xd538_00a0;
const ORR: u32 = 0xaa00_0000;
const UDIV: u32 = 0x9ac0_0800;
const MSUB: u32 = 0x9b00_8000;
const ADD: u32 = 0x9100_0000;
const SUBS_W: u32 = 0x7100_0000;
const AND: u32 = 0x9240_0000;
const LDR_W: u32 = 0xb940_0000;
const STR_W: u32 = 0xb900_0000;
const LDRB_POST_1: u32 = 0x3840_1400;
const RET: u32 = 0xd65f_0000;
const MOVK: u32 = 0xf280_0000;

// The instructions that name a label, with the field of the distance to it.
const B: (u32, Field) = (0x1400_0000, Field::Imm26);
const BL: (u32, Field) = (0x9400_0000, Field::Imm26);
const B_EQ: (u32, Field) = (0x5400_0000, Field::Imm19);
const CBZ: (u32, Field) = (0xb400_0000, Field::Imm19);
const CBNZ: (u32, Field) = (0xb500_0000, Field::Imm19);
const TBNZ: (u32, Field) = (0x3700_0000, Field::Imm14);
const ADR: (u32, Field) = (0x1000_0000, Field::Adr);

fn rrr(op: u32, rd: u32, rn: u32, rm: u32) -> u32 {
    op | rm << 16 | rn << 5 | rd
}

fn rri(op: u32, rd: u32, rn: u32, imm: u32) -> u32 {
    op | imm << 10 | rn << 5 | rd
}

fn movz(rd: u32, imm: u32, shift: u32) -> u32 {
    0xd280_0000 | (shift / 16) << 21 | imm << 5 | rd
}

fn mov(rd: u32, rm: u32) -> u32 {
    rrr(ORR, rd, ZR, rm)
}

/// Where an instruction holds the distance to its label: in words, in bits
/// 0 to 25, 5 to 23 or 5 to 18; or in bytes, over bits 29, 30 and 5 to 23.
#[derive(Clone, Copy)]
enum Field {
    Imm26,
    Imm19,
    Imm14,
    Adr,
}

/// The image as it is written, with its labels and the instructions that
/// wait for the distance to theirs.
#[derive(Default)]
struct Asm {
    bytes: Vec<u8>,
    labels: Vec<(&'static str, usize)>,
    fixups: Vec<(usize, &'static str, Field)>,
}

impl Asm {
    fn word(&mut self, word: u32) {
        self.bytes.extend_from_slice(&word.to_le_bytes());
    }

    fn label(&mut self, name: &'static str) {
        assert!(self.labels.iter().all(|(seen, _)| *seen != name), "{name}");
        self.labels.push((name, self.bytes.len()));
    }

    /// A NUL-terminated text at the label `name`.
    fn text(&mut self, name: &'static str, text: &str) {
        self.label(name);
        self.bytes.extend_from_slice(text.as_bytes());
        self.bytes.push(0);
    }

    /// An instruction naming `label`, its register (and TBNZ's bit at 19) in `operands`.
    fn to(&mut self, (word, field): (u32, Field), operands: u32, label: &'static str) {
        self.fixups.push((self.bytes.len(), label, field));
        self.word(word | operands);
    }

    /// MOVZ and a MOVK for each higher 16 bits of `value` that are not 0.
    fn load(&mut self, rd: u32, value: u64) {
        self.word(movz(rd, (value & 0xffff) as u32, 0));
        for shift in [16, 32, 48] {
            let part = ((value >> shift) & 0xffff) as u32;
            if part != 0 {
                self.word(MOVK | (shift / 16) << 21 | part << 5 | rd);
            }
        }
    }

    /// The image, with the distance to its label in each instruction naming one.
    fn finish(mut self) -> Vec<u8> {
        for (at, label, field) in std::mem::take(&mut self.fixups) {
            let (_, target) = self.labels.iter().find(|(name, _)| *name == label).unwrap();
            let delta = *target as i64 - at as i64;
            let (words, adr) = (delta >> 2, (delta as u32 & 3) << 29);
            let (bits, fits) = match field {
                Field::Imm26 => (words as u32 & 0x3ff_ffff, words.abs() < 1 << 25),
                Field::Imm19 => ((words as u32 & 0x7_ffff) << 5, words.abs() < 1 << 18),
                Field::Imm14 => ((words as u32 & 0x3fff) << 5, words.abs() < 1 << 13),
                Field::Adr => (adr | (words as u32 & 0x7_ffff) << 5, delta.abs() < 1 << 20),
            };
            let aligned = delta % 4 == 0 || matches!(field, Field::Adr);
            assert!(fits && aligned, "{label} is out of reach at {at}");
            let word = u32::from_le_bytes(self.bytes[at..at + 4].try_into().unwrap());
            self.bytes[at..at + 4].copy_from_slice(&(word | bits).to_le_bytes());
        }
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each encoder gives the word that an assembler gives for the
    /// instruction in the comment.
    #[test]
    fn the_encoders_give_the_words_of_the_instructions() {
        let words = [
            (movz(19, 0x1000, 0), 0xd282_0013),          // mov x19, #0x1000
            (movz(20, 0, 0), 0xd280_0014),               // mov x20, #0
            (mov(22, LR), 0xaa1e_03f6),                  // mov x22, x30
            (rrr(ORR, 7, 7, 8), 0xaa08_00e7),            // orr x7, x7, x8
            (rri(AND, 0, 0, 23), 0x9240_5c00),           // and x0, x0, #0xffffff
            (rri(AND, 0, 0, 7), 0x9240_1c00),            // and x0, x0, #0xff
            (rri(ADD, 0, 8, 0x30), 0x9100_c100),         // add x0, x8, #0x30
            (rri(SUBS_W, ZR, 0, 13), 0x7100_341f),       // cmp w0, #13
            (rrr(UDIV, 8, 5, 4), 0x9ac4_08a8),           // udiv x8, x5, x4
            (rrr(MSUB, 5, 8, 4) | 5 << 10, 0x9b04_9505), // msub x5, x8, x4, x5
            (rri(LDR_W, 0, 19, CR), 0xb940_3260),        // ldr w0, [x19, #0x30]
            (rri(STR_W, 0, 19, DR), 0xb900_0260),        // str w0, [x19]
            (rri(LDRB_POST_1, 0, 1, 0), 0x3840_1420),    // ldrb w0, [x1], #1
            (rri(RET, 0, LINK, 0), 0xd65f_02c0),         // ret x22
        ];
        for (word, expected) in words {
            assert_eq!(word, expected, "{word:#010x} is not {expected:#010x}");
        }
        let mut a = Asm::default();
        a.load(4, 10_000_000_000_000_000_000);
        // mov x4, #0; movk x4, #0x89e8, lsl #16; movk x4, #0x2304, lsl #32;
        // movk x4, #0x8ac7, lsl #48
        let loaded: Vec<u32> = a
            .bytes
            .chunks(4)
            .map(|word| u32::from_le_bytes(word.try_into().unwrap()))
            .collect();
        assert_eq!(loaded, [0xd280_0004, 0xf2b1_3d04, 0xf2c4_6084, 0xf2f1_58e4]);
    }

    /// The distance to a label lands in the field of each kind of
    /// instruction, forwards and backwards.
    #[test]
    fn labels_resolve_in_every_field() {
        let mut a = Asm::default();
        a.label("back");
        a.word(NOP);
        a.to(B, 0, "ahead"); // b +12
        a.to(CBZ, 7, "back"); // cbz x7, -8
        a.to(TBNZ, TXFF << 19 | 2, "back"); // tbnz w2, #5, -12
        a.label("ahead");
        a.to(ADR, 1, "text"); // adr x1, +11
        a.word(NOP);
        a.text("pad", "ab");
        a.text("text", "x");
        let image = a.finish();
        let word = |at: usize| u32::from_le_bytes(image[at..at + 4].try_into().unwrap());
        assert_eq!(word(4), 0x1400_0003);
        assert_eq!(word(8), 0xb4ff_ffc7);
        assert_eq!(word(12), 0x372f_ffa2);
        assert_eq!(word(16), 0x7000_0041);
    }

    /// The image starts with the header of an arm64 Linux image whose size
    /// is that of the image, and its code starts right after it.
    #[test]
    fn the_image_has_the_header_of_an_arm64_image() {
        let image = kernel();
        let word = |at: usize| u32::from_le_bytes(image[at..at + 4].try_into().unwrap());
        assert_eq!(word(0), 0x1400_0010, "b 0x40");
        assert_eq!(&image[8..16], &[0; 8], "load offset 0");
        let size = u64::from_le_bytes(image[16..24].try_into().unwrap());
        assert_eq!(size, image.len() as u64);
        assert_eq!(&image[24..56], &[0; 32], "flags and reserved fields 0");
        assert_eq!(&image[56..60], b"ARM\x64");
        assert_eq!(word(64), MRS_X0_MPIDR_EL1);
    }
}
