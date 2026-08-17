/// BasicReal is a four byte number in BASIC MKS$ format.
pub struct BasicReal {
    data: [u8; 4],
}
impl BasicReal {
    pub fn bytes(&self) -> &[u8] {
        &self.data
    }
}

impl From<[u8; 4]> for BasicReal {
    fn from(value: [u8; 4]) -> Self {
        Self { data: value }
    }
}

impl From<Vec<u8>> for BasicReal {
    fn from(value: Vec<u8>) -> Self {
        let mut data = [0u8; 4];
        for (i, slot) in data.iter_mut().enumerate() {
            if let Some(byte) = value.get(i) {
                *slot = *byte;
            }
        }
        Self { data }
    }
}

impl From<i32> for BasicReal {
    fn from(mut value: i32) -> Self {
        let neg = value < 0;
        let mut index = 0;
        if neg {
            value = -value;
        }

        if value > 0xFFFF {
            let high_num = value >> 16;
            while index < 8 {
                if (1 << index) > high_num {
                    break;
                }
                index += 1;
            }
            index += 16;
        } else {
            while index < 16 {
                if (1 << index) > value {
                    break;
                }
                index += 1;
            }
        }

        let value = value << (24 - index);

        let mut data = [0u8; 4];
        data[3] = (index + 0x80) as u8;
        data[2] = (value >> 16) as u8 & 0x7F;
        data[1] = (value >> 8) as u8;
        data[0] = value as u8;

        if neg {
            data[2] |= 0x80;
        }
        Self { data }
    }
}

impl From<BasicReal> for i32 {
    fn from(value: BasicReal) -> i32 {
        let exponent = value.data[3] as i16;
        if exponent == 0 || exponent == 0x80 {
            return 0;
        }
        let is_negative = (value.data[2] & 0x80) != 0;

        let value = (u32::from_le_bytes(value.data) & 0x7F_FFFF) as i32 | 0x80_0000;

        let exponent = exponent - 152;
        let result = if exponent < 0 {
            value.wrapping_shr((-exponent) as u32)
        } else {
            value.wrapping_shl(exponent as u32)
        };

        if is_negative {
            if result == i32::MIN {
                return 1;
            }
            -result
        } else {
            result
        }
    }
}

impl From<BasicReal> for u32 {
    fn from(value: BasicReal) -> u32 {
        let value: i32 = value.into();
        value as u32
    }
}

/// BasicDouble is a 8 byte number in BASIC MKS$ format.
pub struct BasicDouble {
    data: [u8; 8],
}
impl BasicDouble {
    pub fn bytes(&self) -> &[u8] {
        &self.data
    }
}

impl From<[u8; 8]> for BasicDouble {
    fn from(value: [u8; 8]) -> Self {
        Self { data: value }
    }
}

impl From<Vec<u8>> for BasicDouble {
    fn from(value: Vec<u8>) -> Self {
        let mut data = [0u8; 8];
        for (i, slot) in data.iter_mut().enumerate() {
            if let Some(byte) = value.get(i) {
                *slot = *byte;
            }
        }
        Self { data }
    }
}

impl From<i64> for BasicDouble {
    fn from(mut value: i64) -> Self {
        let neg = value < 0;
        let mut index = 0;
        if neg {
            value = -value;
        }
        if value > 0xFFFF {
            let high_num = value >> 16;
            while index < 16 {
                if (1 << index) > high_num {
                    break;
                }
                index += 1;
            }
            index += 16;
        } else {
            while index < 16 {
                if (1 << index) > value {
                    break;
                }
                index += 1;
            }
        }

        let value = value << (32 - index);
        let mut data = [
            (index + 0x80) as u8,
            (value >> 24) as u8 & 0x7F,
            (value >> 16) as u8,
            (value >> 8) as u8,
            value as u8,
            0,
            0,
            0,
        ];

        if neg {
            data[6] |= 0x80;
        }
        Self { data }
    }
}

impl From<BasicDouble> for i64 {
    fn from(value: BasicDouble) -> i64 {
        let exponent = value.data[7] as i16;
        if exponent == 0 || exponent == 0x80 {
            return 0;
        }
        let is_negative = (value.data[6] & 0x80) != 0;

        let value =
            ((u64::from_le_bytes(value.data) & 0x007FFFFF_FFFFFFFF) | 0x00800000_00000000) as i64;

        let exponent = exponent - 152;
        let result = if exponent < 0 {
            value.wrapping_shr((-exponent) as u32)
        } else {
            value.wrapping_shl(exponent as u32)
        };

        if is_negative {
            if result == i64::MIN {
                return 1;
            }
            -result
        } else {
            result
        }
    }
}
