#[macro_export]
macro_rules! convert_u32 {
    ( $t:ident, $x:expr ) => {
        let $t = $x[0] as u32 | ($x[1] as u32) << 8 | ($x[2] as u32) << 16 | ($x[3] as u32) << 24;

        #[allow(unused_assignments)]
        {
            $x = &$x[4..];
        }
    };
}

// same as convert_u32 but moves 5 bytes instead of 4, omitting unused conference number.
#[macro_export]
macro_rules! convert_qwk_ndx {
    ( $t:ident, $x:expr ) => {
        let $t = $x[0] as u32 | ($x[1] as u32) << 8 | ($x[2] as u32) << 16 | ($x[3] as u32) << 24;

        #[allow(unused_assignments)]
        {
            $x = &$x[5..];
        }
    };
}

#[macro_export]
macro_rules! convert_u16 {
    ( $t:ident, $x:expr ) => {
        let $t = $x[0] as u16 | ($x[1] as u16) << 8;

        #[allow(unused_assignments)]
        {
            $x = &$x[2..];
        }
    };
}

#[macro_export]
macro_rules! convert_u8 {
    ( $t:ident, $x:expr ) => {
        let $t = $x[0];

        #[allow(unused_assignments)]
        {
            $x = &$x[1..];
        }
    };
}

#[macro_export]
macro_rules! convert_single_u32 {
    ( $t:ident, $x:expr ) => {
        let $t = $x[0] as u32 | ($x[1] as u32) << 8 | ($x[2] as u32) << 16 | ($x[3] as u32) << 24;
    };
}

#[macro_export]
macro_rules! convert_single_u16 {
    ( $t:ident, $x:expr ) => {
        let $t = $x[0] as u16 | ($x[1] as u16) << 8;
    };
}

#[macro_export]
macro_rules! convert_buffer {
    ( $t:ident, $x:expr, $len:tt) => {
        let $t = $x[0..$len];

        #[allow(unused_assignments)]
        {
            $x = &$x[$len..];
        }
    };
}

#[macro_export]
macro_rules! convert_to_string {
    ( $t:ident, $x:expr, $len:expr ) => {
        let $t = convert_str(&$x[0..$len]);

        #[allow(unused_assignments)]
        {
            $x = &$x[$len..];
        }
    };
}
