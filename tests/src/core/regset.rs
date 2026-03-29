use machina_core::types::RegSet;

macro_rules! regset_set_contains_tests {
    ($( $name:ident: $reg:expr, )+ $(,)?) => {
        $(
            #[test]
            fn $name() {
                let reg: u8 = $reg;
                let s = RegSet::EMPTY.set(reg);
                assert!(s.contains(reg));
                assert_eq!(s.count(), 1);
                assert_eq!(s.first(), Some(reg));
            }
        )+
    };
}

macro_rules! regset_clear_tests {
    ($( $name:ident: $reg:expr, )+ $(,)?) => {
        $(
            #[test]
            fn $name() {
                let reg: u8 = $reg;
                let next: u8 = (reg + 1) & 63;
                let s = RegSet::EMPTY.set(reg).set(next);
                let c = s.clear(reg);
                assert!(!c.contains(reg));
                assert!(c.contains(next));
                assert_eq!(c.count(), 1);
            }
        )+
    };
}

regset_set_contains_tests! {
    regset_set_contains_0: 0,
    regset_set_contains_1: 1,
    regset_set_contains_2: 2,
    regset_set_contains_3: 3,
    regset_set_contains_4: 4,
    regset_set_contains_5: 5,
    regset_set_contains_6: 6,
    regset_set_contains_7: 7,
    regset_set_contains_8: 8,
    regset_set_contains_9: 9,
    regset_set_contains_10: 10,
    regset_set_contains_11: 11,
    regset_set_contains_12: 12,
    regset_set_contains_13: 13,
    regset_set_contains_14: 14,
    regset_set_contains_15: 15,
    regset_set_contains_16: 16,
    regset_set_contains_17: 17,
    regset_set_contains_18: 18,
    regset_set_contains_19: 19,
    regset_set_contains_20: 20,
    regset_set_contains_21: 21,
    regset_set_contains_22: 22,
    regset_set_contains_23: 23,
    regset_set_contains_24: 24,
    regset_set_contains_25: 25,
    regset_set_contains_26: 26,
    regset_set_contains_27: 27,
    regset_set_contains_28: 28,
    regset_set_contains_29: 29,
    regset_set_contains_30: 30,
    regset_set_contains_31: 31,
    regset_set_contains_32: 32,
    regset_set_contains_33: 33,
    regset_set_contains_34: 34,
    regset_set_contains_35: 35,
    regset_set_contains_36: 36,
    regset_set_contains_37: 37,
    regset_set_contains_38: 38,
    regset_set_contains_39: 39,
    regset_set_contains_40: 40,
    regset_set_contains_41: 41,
    regset_set_contains_42: 42,
    regset_set_contains_43: 43,
    regset_set_contains_44: 44,
    regset_set_contains_45: 45,
    regset_set_contains_46: 46,
    regset_set_contains_47: 47,
    regset_set_contains_48: 48,
    regset_set_contains_49: 49,
    regset_set_contains_50: 50,
    regset_set_contains_51: 51,
    regset_set_contains_52: 52,
    regset_set_contains_53: 53,
    regset_set_contains_54: 54,
    regset_set_contains_55: 55,
    regset_set_contains_56: 56,
    regset_set_contains_57: 57,
    regset_set_contains_58: 58,
    regset_set_contains_59: 59,
    regset_set_contains_60: 60,
    regset_set_contains_61: 61,
    regset_set_contains_62: 62,
    regset_set_contains_63: 63,
}

regset_clear_tests! {
    regset_clear_0: 0,
    regset_clear_1: 1,
    regset_clear_2: 2,
    regset_clear_3: 3,
    regset_clear_4: 4,
    regset_clear_5: 5,
    regset_clear_6: 6,
    regset_clear_7: 7,
    regset_clear_8: 8,
    regset_clear_9: 9,
    regset_clear_10: 10,
    regset_clear_11: 11,
    regset_clear_12: 12,
    regset_clear_13: 13,
    regset_clear_14: 14,
    regset_clear_15: 15,
    regset_clear_16: 16,
    regset_clear_17: 17,
    regset_clear_18: 18,
    regset_clear_19: 19,
    regset_clear_20: 20,
    regset_clear_21: 21,
    regset_clear_22: 22,
    regset_clear_23: 23,
    regset_clear_24: 24,
    regset_clear_25: 25,
    regset_clear_26: 26,
    regset_clear_27: 27,
    regset_clear_28: 28,
    regset_clear_29: 29,
    regset_clear_30: 30,
    regset_clear_31: 31,
    regset_clear_32: 32,
    regset_clear_33: 33,
    regset_clear_34: 34,
    regset_clear_35: 35,
    regset_clear_36: 36,
    regset_clear_37: 37,
    regset_clear_38: 38,
    regset_clear_39: 39,
    regset_clear_40: 40,
    regset_clear_41: 41,
    regset_clear_42: 42,
    regset_clear_43: 43,
    regset_clear_44: 44,
    regset_clear_45: 45,
    regset_clear_46: 46,
    regset_clear_47: 47,
    regset_clear_48: 48,
    regset_clear_49: 49,
    regset_clear_50: 50,
    regset_clear_51: 51,
    regset_clear_52: 52,
    regset_clear_53: 53,
    regset_clear_54: 54,
    regset_clear_55: 55,
    regset_clear_56: 56,
    regset_clear_57: 57,
    regset_clear_58: 58,
    regset_clear_59: 59,
    regset_clear_60: 60,
    regset_clear_61: 61,
    regset_clear_62: 62,
    regset_clear_63: 63,
}
