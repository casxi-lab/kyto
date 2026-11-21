//! The self-test kernel run on a model of the machine it is written for:
//! the instructions that it is made of, the core's MPIDR and the PL011
//! registers that it uses. The model knows only those instructions, so an
//! instruction it does not know fails the test, and so does a load or store
//! outside the image and those registers, a store to the UART other than to
//! its data or control register, a write to the data register while the
//! transmit FIFO is full or transmission is off, and a read of it while
//! reception is off or nothing has been received. The transmit FIFO sends
//! a byte for every few reads of the flag register, more slowly than the
//! kernel writes, so the kernel has to wait for it. Received bytes can carry
//! the error bits above the byte. The image runs at the offset at which the
//! firmware of a bench loads it, and at another, since the kernel does not
//! depend on that.

use std::collections::VecDeque;

/// The offset at which the firmware of a bench loads the kernel.
const LOAD: u64 = 0x20_0000;
const DR: u64 = kyto_self_test::UART;
const FR: u64 = kyto_self_test::UART + 0x18;
const CR: u64 = kyto_self_test::UART + 0x30;

/// The flags of FR: receive FIFO empty, transmit FIFO full and transmit
/// FIFO empty.
const RXFE: u32 = 1 << 4;
const TXFF: u32 = 1 << 5;
const TXFE: u32 = 1 << 7;
/// The bits of CR that enable the UART, transmission and reception.
const UARTEN: u32 = 1;
const TXE: u32 = 1 << 8;
const RXE: u32 = 1 << 9;
/// The depth of the PL011's transmit FIFO.
const FIFO: usize = 32;
/// The transmit FIFO sends one byte for this many reads of FR.
const SEND_EVERY: u32 = 3;

/// The MPIDR_EL1 of core 0 of a Cortex-A76 cluster: the RES1 bit 31, the
/// MT bit 24, and zero in Aff0 to Aff3.
const CORE0: u64 = 0x8100_0000;

/// How the kernel stopped: polling a receive FIFO that stays empty, or
/// parked in WFE.
#[derive(Debug, PartialEq)]
enum Stop {
    Waiting,
    Parked,
}

struct Machine {
    image: Vec<u8>,
    base: u64,
    x: [u64; 31],
    pc: u64,
    zero: bool,
    /// The event register, which the exception return into the kernel sets.
    event: bool,
    mpidr: u64,
    cr: u32,
    rx: VecDeque<u8>,
    input: VecDeque<u8>,
    /// The error bits that come with every received byte.
    errors: u32,
    tx: Vec<u8>,
    /// The bytes in the transmit FIFO, and whether it has been full.
    queued: usize,
    filled: bool,
    fr_reads: u32,
    idle_polls: u32,
    uart_accesses: u32,
}

impl Machine {
    /// The kernel as it starts, with `early` already received and `input`
    /// arriving once the ready line has been written.
    fn new(mpidr: u64, cr: u32, early: &[u8], input: &[u8]) -> Self {
        Self {
            image: kyto_self_test::kernel(),
            base: LOAD,
            x: [0; 31],
            pc: LOAD,
            zero: false,
            event: true,
            mpidr,
            cr,
            rx: early.iter().copied().collect(),
            input: input.iter().copied().collect(),
            errors: 0,
            tx: Vec::new(),
            queued: 0,
            filled: false,
            fr_reads: 0,
            idle_polls: 0,
            uart_accesses: 0,
        }
    }

    /// The same, with the image at `base`.
    fn at(mut self, base: u64) -> Self {
        self.base = base;
        self.pc = base;
        self
    }

    /// The same, with `errors` set in every byte read from DR.
    fn with_errors(mut self, errors: u32) -> Self {
        self.errors = errors;
        self
    }

    fn reg(&self, r: u32) -> u64 {
        self.x.get(r as usize).copied().unwrap_or(0)
    }

    fn set(&mut self, r: u32, value: u64) {
        if let Some(x) = self.x.get_mut(r as usize) {
            *x = value;
        }
    }

    fn byte(&self, address: u64) -> u8 {
        let at = address.checked_sub(self.base).map(|at| at as usize);
        let byte = at.and_then(|at| self.image.get(at));
        *byte.unwrap_or_else(|| panic!("a load from {address:#x}"))
    }

    fn word(&self, address: u64) -> u32 {
        u32::from_le_bytes([0, 1, 2, 3].map(|i| self.byte(address + i)))
    }

    fn load(&mut self, address: u64) -> u32 {
        self.uart_accesses += 1;
        match address {
            FR => {
                self.fr_reads += 1;
                if self.fr_reads.is_multiple_of(SEND_EVERY) {
                    self.queued = self.queued.saturating_sub(1);
                }
                if self.rx.is_empty() && self.queued == 0 {
                    self.idle_polls += 1;
                }
                let flag = |set: bool, flag: u32| if set { flag } else { 0 };
                flag(self.rx.is_empty(), RXFE)
                    | flag(self.queued == FIFO, TXFF)
                    | flag(self.queued == 0, TXFE)
            }
            DR => {
                let on = UARTEN | RXE;
                assert_eq!(self.cr & on, on, "a read of DR with reception off");
                let byte = self
                    .rx
                    .pop_front()
                    .expect("a read of DR with nothing received");
                u32::from(byte) | self.errors
            }
            CR => self.cr,
            _ => panic!("a load from {address:#x}"),
        }
    }

    fn store(&mut self, address: u64, value: u32) {
        self.uart_accesses += 1;
        match address {
            DR => {
                let on = UARTEN | TXE;
                assert_eq!(self.cr & on, on, "a write to DR with transmission off");
                assert!(
                    self.queued < FIFO,
                    "a write to DR with the transmit FIFO full"
                );
                self.queued += 1;
                self.filled |= self.queued == FIFO;
                self.tx.push(value as u8);
                if self.tx.ends_with(b"kyto self-test: ready\r\n") {
                    self.rx.extend(self.input.drain(..));
                }
            }
            CR => self.cr = value,
            _ => panic!("a store of {value:#x} to {address:#x}"),
        }
    }

    /// Runs until the kernel polls an empty receive FIFO with the transmit
    /// FIFO empty, or parks: reaches WFE once the event register is clear.
    fn run(&mut self) -> Stop {
        for _ in 0..10_000_000 {
            if self.idle_polls > 100 {
                return Stop::Waiting;
            }
            let w = self.word(self.pc);
            let (rd, rn, rm) = (w & 31, w >> 5 & 31, w >> 16 & 31);
            let pc = self.pc;
            let mut next = pc + 4;
            let offset = |bits: u32, at: u32| sext(u64::from(w >> at), bits) << 2;
            match w {
                0xd503_201f => {}
                0xd503_205f if self.event => self.event = false,
                0xd503_205f => return Stop::Parked,
                _ if w & !31 == 0xd538_00a0 => self.set(rd, self.mpidr),
                _ if w & 0xfc00_0000 == 0x1400_0000 => next = pc.wrapping_add(offset(26, 0)),
                _ if w & 0xfc00_0000 == 0x9400_0000 => {
                    self.set(30, next);
                    next = pc.wrapping_add(offset(26, 0));
                }
                _ if w & 0xff00_001f == 0x5400_0000 && self.zero => {
                    next = pc.wrapping_add(offset(19, 5));
                }
                _ if w & 0xff00_001f == 0x5400_0000 => {}
                _ if w & 0xfe00_0000 == 0xb400_0000 => {
                    if (self.reg(rd) == 0) == (w & 1 << 24 == 0) {
                        next = pc.wrapping_add(offset(19, 5));
                    }
                }
                _ if w & 0x7f00_0000 == 0x3700_0000 => {
                    let bit = (w >> 31) << 5 | (w >> 19 & 31);
                    if self.reg(rd) >> bit & 1 == 1 {
                        next = pc.wrapping_add(offset(14, 5));
                    }
                }
                _ if w & 0x9f00_0000 == 0x1000_0000 => {
                    let imm = u64::from((w >> 5 & 0x7_ffff) << 2 | (w >> 29 & 3));
                    self.set(rd, pc.wrapping_add(sext(imm, 21)));
                }
                _ if w & 0xffe0_fc00 == 0xaa00_0000 => self.set(rd, self.reg(rn) | self.reg(rm)),
                _ if w & 0xffe0_fc00 == 0x9ac0_0800 => {
                    let divisor = self.reg(rm);
                    self.set(rd, self.reg(rn).checked_div(divisor).unwrap_or(0));
                }
                _ if w & 0xffe0_8000 == 0x9b00_8000 => {
                    let product = self.reg(rn).wrapping_mul(self.reg(rm));
                    self.set(rd, self.reg(w >> 10 & 31).wrapping_sub(product));
                }
                _ if w & 0xffc0_0000 == 0x9100_0000 && rd != 31 && rn != 31 => {
                    self.set(rd, self.reg(rn) + u64::from(w >> 10 & 0xfff));
                }
                _ if w & 0xffc0_0000 == 0x7100_0000 && rd == 31 && rn != 31 => {
                    self.zero = self.reg(rn) as u32 == w >> 10 & 0xfff;
                }
                _ if w & 0xffff_0000 == 0x9240_0000 && w >> 10 & 63 < 63 && rd != 31 => {
                    let ones = (w >> 10 & 63) + 1;
                    self.set(rd, self.reg(rn) & ((1 << ones) - 1));
                }
                _ if w & 0xffc0_0000 == 0xb940_0000 && rn != 31 => {
                    let value = self.load(self.reg(rn) + u64::from(w >> 10 & 0xfff) * 4);
                    self.set(rd, u64::from(value));
                }
                _ if w & 0xffc0_0000 == 0xb900_0000 && rn != 31 => {
                    let address = self.reg(rn) + u64::from(w >> 10 & 0xfff) * 4;
                    self.store(address, self.reg(rd) as u32);
                }
                _ if w & 0xffe0_0c00 == 0x3840_0400 && rn != 31 => {
                    let base = self.reg(rn);
                    self.set(rd, u64::from(self.byte(base)));
                    self.set(rn, base.wrapping_add(sext(u64::from(w >> 12 & 0x1ff), 9)));
                }
                _ if w & 0xffff_fc1f == 0xd65f_0000 => next = self.reg(rn),
                _ if w & 0xff80_0000 == 0xd280_0000 => {
                    self.set(rd, u64::from(w >> 5 & 0xffff) << (16 * (w >> 21 & 3)));
                }
                _ if w & 0xff80_0000 == 0xf280_0000 => {
                    let shift = 16 * (w >> 21 & 3);
                    let kept = self.reg(rd) & !(0xffff << shift);
                    self.set(rd, kept | u64::from(w >> 5 & 0xffff) << shift);
                }
                _ => panic!("an instruction the model does not know: {w:#010x} at {pc:#x}"),
            }
            self.pc = next;
        }
        panic!("the kernel neither waited nor parked")
    }

    fn output(&self) -> String {
        String::from_utf8(self.tx.clone()).expect("the kernel writes text")
    }
}

/// `value` of `bits` bits, sign-extended to 64.
fn sext(value: u64, bits: u32) -> u64 {
    let unused = 64 - bits;
    (((value << unused) as i64) >> unused) as u64
}

fn answer(line: &str) -> String {
    format!("kyto self-test: echo {line} ({} bytes)\r\n", line.len())
}

const READY: &str = "\r\nkyto self-test: ready\r\n";

#[test]
fn the_kernel_enables_the_uart_discards_what_came_before_and_answers_a_line() {
    // The control register starts with the UART, transmission and
    // reception off, so a write or read of DR before they are on fails.
    for base in [LOAD, 0x8_0000] {
        let mut machine = Machine::new(CORE0, 0x70, b"noise", b"hello bench\r").at(base);
        assert_eq!(machine.run(), Stop::Waiting, "{base:#x}");
        assert_eq!(machine.cr, 0x70 | UARTEN | TXE | RXE, "{base:#x}");
        let expected = format!("{READY}{}", answer("hello bench"));
        assert_eq!(machine.output(), expected, "{base:#x}");
    }
    assert_eq!(
        answer("hello bench"),
        "kyto self-test: echo hello bench (11 bytes)\r\n"
    );
}

#[test]
fn the_error_bits_of_a_received_byte_are_not_part_of_it() {
    // Framing, parity, break and overrun errors on every byte.
    let machine = Machine::new(CORE0, 0x301, b"", b"hello bench\r");
    let mut machine = machine.with_errors(0xf00);
    assert_eq!(machine.run(), Stop::Waiting);
    assert_eq!(
        machine.output(),
        format!("{READY}{}", answer("hello bench"))
    );
}

#[test]
fn cr_or_lf_ends_a_line_and_an_empty_line_gets_no_answer() {
    let mut machine = Machine::new(CORE0, 0x301, b"", b"a\r\n\r\nbc\n\n");
    assert_eq!(machine.run(), Stop::Waiting);
    assert_eq!(
        machine.output(),
        format!("{READY}{}{}", answer("a"), answer("bc"))
    );
}

#[test]
fn the_count_of_bytes_is_written_in_decimal() {
    let lines: Vec<String> = [1, 9, 10, 99, 100, 1000, 12345]
        .iter()
        .map(|&n| "x".repeat(n))
        .collect();
    let input: String = lines.iter().map(|line| format!("{line}\r")).collect();
    let mut machine = Machine::new(CORE0, 0x301, b"", input.as_bytes());
    assert_eq!(machine.run(), Stop::Waiting);
    let answers: String = lines.iter().map(|line| answer(line)).collect();
    assert_eq!(machine.output(), format!("{READY}{answers}"));
    assert!(machine.filled, "the kernel waited for a full transmit FIFO");
}

#[test]
fn every_core_but_core_0_of_cluster_0_parks_without_touching_the_uart() {
    for mpidr in [CORE0 | 1, CORE0 | 0x100, CORE0 | 0x300, CORE0 | 0x1_0000] {
        let mut machine = Machine::new(mpidr, 0x70, b"", b"hello\r");
        assert_eq!(machine.run(), Stop::Parked, "{mpidr:#x}");
        assert_eq!(machine.uart_accesses, 0, "{mpidr:#x}");
    }
    // Aff3 and the bits above Aff2 do not count.
    let mut machine = Machine::new(0xff_8100_0000, 0x301, b"", b"");
    assert_eq!(machine.run(), Stop::Waiting);
    assert_eq!(machine.output(), READY);
}
