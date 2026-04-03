//! Cross-platform key code conversion via USB HID Usage Codes.
//!
//! The wire protocol's `hid_usage` field carries USB HID Usage Codes (Page 0x07).
//! Each platform converts to/from its native key codes at capture/injection time.

/// Convert a Windows Virtual Key code to USB HID Usage Code.
pub fn vkey_to_hid(vkey: u16) -> Option<u32> {
    VKEY_TO_HID
        .iter()
        .find(|&&(v, _)| v == vkey)
        .map(|&(_, h)| h)
}

/// Convert a USB HID Usage Code to Windows Virtual Key code.
pub fn hid_to_vkey(hid: u32) -> Option<u16> {
    VKEY_TO_HID
        .iter()
        .find(|&&(_, h)| h == hid)
        .map(|&(v, _)| v)
}

/// Convert a macOS CGKeyCode to USB HID Usage Code.
pub fn cgkeycode_to_hid(code: u16) -> Option<u32> {
    CG_TO_HID.iter().find(|&&(c, _)| c == code).map(|&(_, h)| h)
}

/// Convert a USB HID Usage Code to macOS CGKeyCode.
pub fn hid_to_cgkeycode(hid: u32) -> Option<u16> {
    CG_TO_HID.iter().find(|&&(_, h)| h == hid).map(|&(c, _)| c)
}

/// (Windows VKey, USB HID Usage Code) mapping table.
///
/// Sources:
/// - VKey: https://learn.microsoft.com/en-us/windows/win32/inputdev/virtual-key-codes
/// - HID:  USB HID Usage Tables v1.4, Section 10 (Keyboard/Keypad Page 0x07)
static VKEY_TO_HID: &[(u16, u32)] = &[
    // Letters A-Z (VK_A=0x41 .. VK_Z=0x5A -> HID 0x04..0x1D)
    (0x41, 0x04), // A
    (0x42, 0x05), // B
    (0x43, 0x06), // C
    (0x44, 0x07), // D
    (0x45, 0x08), // E
    (0x46, 0x09), // F
    (0x47, 0x0A), // G
    (0x48, 0x0B), // H
    (0x49, 0x0C), // I
    (0x4A, 0x0D), // J
    (0x4B, 0x0E), // K
    (0x4C, 0x0F), // L
    (0x4D, 0x10), // M
    (0x4E, 0x11), // N
    (0x4F, 0x12), // O
    (0x50, 0x13), // P
    (0x51, 0x14), // Q
    (0x52, 0x15), // R
    (0x53, 0x16), // S
    (0x54, 0x17), // T
    (0x55, 0x18), // U
    (0x56, 0x19), // V
    (0x57, 0x1A), // W
    (0x58, 0x1B), // X
    (0x59, 0x1C), // Y
    (0x5A, 0x1D), // Z
    // Digits 0-9 (VK_0=0x30 .. VK_9=0x39 -> HID 0x27, 0x1E..0x26)
    (0x31, 0x1E), // 1
    (0x32, 0x1F), // 2
    (0x33, 0x20), // 3
    (0x34, 0x21), // 4
    (0x35, 0x22), // 5
    (0x36, 0x23), // 6
    (0x37, 0x24), // 7
    (0x38, 0x25), // 8
    (0x39, 0x26), // 9
    (0x30, 0x27), // 0
    // Editing keys
    (0x0D, 0x28), // VK_RETURN -> Enter
    (0x1B, 0x29), // VK_ESCAPE -> Escape
    (0x08, 0x2A), // VK_BACK -> Backspace
    (0x09, 0x2B), // VK_TAB -> Tab
    (0x20, 0x2C), // VK_SPACE -> Space
    // Symbols
    (0xBD, 0x2D), // VK_OEM_MINUS -> - _
    (0xBB, 0x2E), // VK_OEM_PLUS -> = +
    (0xDB, 0x2F), // VK_OEM_4 -> [ {
    (0xDD, 0x30), // VK_OEM_6 -> ] }
    (0xDC, 0x31), // VK_OEM_5 -> \ |
    (0xBA, 0x33), // VK_OEM_1 -> ; :
    (0xDE, 0x34), // VK_OEM_7 -> ' "
    (0xC0, 0x35), // VK_OEM_3 -> ` ~
    (0xBC, 0x36), // VK_OEM_COMMA -> , <
    (0xBE, 0x37), // VK_OEM_PERIOD -> . >
    (0xBF, 0x38), // VK_OEM_2 -> / ?
    // Lock keys
    (0x14, 0x39), // VK_CAPITAL -> Caps Lock
    // Function keys F1-F12
    (0x70, 0x3A), // VK_F1
    (0x71, 0x3B), // VK_F2
    (0x72, 0x3C), // VK_F3
    (0x73, 0x3D), // VK_F4
    (0x74, 0x3E), // VK_F5
    (0x75, 0x3F), // VK_F6
    (0x76, 0x40), // VK_F7
    (0x77, 0x41), // VK_F8
    (0x78, 0x42), // VK_F9
    (0x79, 0x43), // VK_F10
    (0x7A, 0x44), // VK_F11
    (0x7B, 0x45), // VK_F12
    // System keys
    (0x2C, 0x46), // VK_SNAPSHOT -> PrintScreen
    (0x91, 0x47), // VK_SCROLL -> Scroll Lock
    (0x13, 0x48), // VK_PAUSE -> Pause
    // Navigation keys
    (0x2D, 0x49), // VK_INSERT -> Insert
    (0x24, 0x4A), // VK_HOME -> Home
    (0x21, 0x4B), // VK_PRIOR -> Page Up
    (0x2E, 0x4C), // VK_DELETE -> Delete Forward
    (0x23, 0x4D), // VK_END -> End
    (0x22, 0x4E), // VK_NEXT -> Page Down
    // Arrow keys
    (0x27, 0x4F), // VK_RIGHT -> Right Arrow
    (0x25, 0x50), // VK_LEFT -> Left Arrow
    (0x28, 0x51), // VK_DOWN -> Down Arrow
    (0x26, 0x52), // VK_UP -> Up Arrow
    // Keypad
    (0x90, 0x53), // VK_NUMLOCK -> Num Lock
    (0x6F, 0x54), // VK_DIVIDE -> Keypad /
    (0x6A, 0x55), // VK_MULTIPLY -> Keypad *
    (0x6D, 0x56), // VK_SUBTRACT -> Keypad -
    (0x6B, 0x57), // VK_ADD -> Keypad +
    // Note: VK_RETURN (0x0D) is shared with main Enter -- keypad Enter is
    // distinguished by extended key flag, not VKey. Map separately if needed.
    (0x60, 0x62), // VK_NUMPAD0
    (0x61, 0x59), // VK_NUMPAD1
    (0x62, 0x5A), // VK_NUMPAD2
    (0x63, 0x5B), // VK_NUMPAD3
    (0x64, 0x5C), // VK_NUMPAD4
    (0x65, 0x5D), // VK_NUMPAD5
    (0x66, 0x5E), // VK_NUMPAD6
    (0x67, 0x5F), // VK_NUMPAD7
    (0x68, 0x60), // VK_NUMPAD8
    (0x69, 0x61), // VK_NUMPAD9
    (0x6E, 0x63), // VK_DECIMAL -> Keypad .
    // Modifier keys
    (0xA0, 0xE1), // VK_LSHIFT -> Left Shift
    (0xA1, 0xE5), // VK_RSHIFT -> Right Shift
    (0xA2, 0xE0), // VK_LCONTROL -> Left Control
    (0xA3, 0xE4), // VK_RCONTROL -> Right Control
    (0xA4, 0xE2), // VK_LMENU -> Left Alt
    (0xA5, 0xE6), // VK_RMENU -> Right Alt
    (0x5B, 0xE3), // VK_LWIN -> Left GUI (Meta/Super)
    (0x5C, 0xE7), // VK_RWIN -> Right GUI (Meta/Super)
];

/// (macOS CGKeyCode, USB HID Usage Code) mapping table.
///
/// Sources:
/// - CGKeyCode: Carbon Events.h (kVK_* constants)
/// - HID:  USB HID Usage Tables v1.4, Section 10 (Keyboard/Keypad Page 0x07)
static CG_TO_HID: &[(u16, u32)] = &[
    // Letters (macOS keyboard layout follows physical QWERTY positions)
    (0x00, 0x04), // kVK_ANSI_A
    (0x0B, 0x05), // kVK_ANSI_B
    (0x08, 0x06), // kVK_ANSI_C
    (0x02, 0x07), // kVK_ANSI_D
    (0x0E, 0x08), // kVK_ANSI_E
    (0x03, 0x09), // kVK_ANSI_F
    (0x05, 0x0A), // kVK_ANSI_G
    (0x04, 0x0B), // kVK_ANSI_H
    (0x22, 0x0C), // kVK_ANSI_I
    (0x26, 0x0D), // kVK_ANSI_J
    (0x28, 0x0E), // kVK_ANSI_K
    (0x25, 0x0F), // kVK_ANSI_L
    (0x2E, 0x10), // kVK_ANSI_M
    (0x2D, 0x11), // kVK_ANSI_N
    (0x1F, 0x12), // kVK_ANSI_O
    (0x23, 0x13), // kVK_ANSI_P
    (0x0C, 0x14), // kVK_ANSI_Q
    (0x0F, 0x15), // kVK_ANSI_R
    (0x01, 0x16), // kVK_ANSI_S
    (0x11, 0x17), // kVK_ANSI_T
    (0x20, 0x18), // kVK_ANSI_U
    (0x09, 0x19), // kVK_ANSI_V
    (0x0D, 0x1A), // kVK_ANSI_W
    (0x07, 0x1B), // kVK_ANSI_X
    (0x10, 0x1C), // kVK_ANSI_Y
    (0x06, 0x1D), // kVK_ANSI_Z
    // Digits
    (0x12, 0x1E), // kVK_ANSI_1
    (0x13, 0x1F), // kVK_ANSI_2
    (0x14, 0x20), // kVK_ANSI_3
    (0x15, 0x21), // kVK_ANSI_4
    (0x17, 0x22), // kVK_ANSI_5
    (0x16, 0x23), // kVK_ANSI_6
    (0x1A, 0x24), // kVK_ANSI_7
    (0x1C, 0x25), // kVK_ANSI_8
    (0x19, 0x26), // kVK_ANSI_9
    (0x1D, 0x27), // kVK_ANSI_0
    // Editing keys
    (0x24, 0x28), // kVK_Return
    (0x35, 0x29), // kVK_Escape
    (0x33, 0x2A), // kVK_Delete (Backspace)
    (0x30, 0x2B), // kVK_Tab
    (0x31, 0x2C), // kVK_Space
    // Symbols
    (0x1B, 0x2D), // kVK_ANSI_Minus
    (0x18, 0x2E), // kVK_ANSI_Equal
    (0x21, 0x2F), // kVK_ANSI_LeftBracket
    (0x1E, 0x30), // kVK_ANSI_RightBracket
    (0x2A, 0x31), // kVK_ANSI_Backslash
    (0x29, 0x33), // kVK_ANSI_Semicolon
    (0x27, 0x34), // kVK_ANSI_Quote
    (0x32, 0x35), // kVK_ANSI_Grave
    (0x2B, 0x36), // kVK_ANSI_Comma
    (0x2F, 0x37), // kVK_ANSI_Period
    (0x2C, 0x38), // kVK_ANSI_Slash
    // Lock keys
    (0x39, 0x39), // kVK_CapsLock
    // Function keys F1-F12
    (0x7A, 0x3A), // kVK_F1
    (0x78, 0x3B), // kVK_F2
    (0x63, 0x3C), // kVK_F3
    (0x76, 0x3D), // kVK_F4
    (0x60, 0x3E), // kVK_F5
    (0x61, 0x3F), // kVK_F6
    (0x62, 0x40), // kVK_F7
    (0x64, 0x41), // kVK_F8
    (0x65, 0x42), // kVK_F9
    (0x6D, 0x43), // kVK_F10
    (0x67, 0x44), // kVK_F11
    (0x6F, 0x45), // kVK_F12
    // Navigation keys
    (0x72, 0x49), // kVK_Help -> Insert (Mac "Help" key = PC "Insert")
    (0x73, 0x4A), // kVK_Home
    (0x74, 0x4B), // kVK_PageUp
    (0x75, 0x4C), // kVK_ForwardDelete
    (0x77, 0x4D), // kVK_End
    (0x79, 0x4E), // kVK_PageDown
    // Arrow keys
    (0x7C, 0x4F), // kVK_RightArrow
    (0x7B, 0x50), // kVK_LeftArrow
    (0x7D, 0x51), // kVK_DownArrow
    (0x7E, 0x52), // kVK_UpArrow
    // Keypad
    (0x47, 0x53), // kVK_ANSI_KeypadClear -> Num Lock
    (0x4B, 0x54), // kVK_ANSI_KeypadDivide
    (0x43, 0x55), // kVK_ANSI_KeypadMultiply
    (0x4E, 0x56), // kVK_ANSI_KeypadMinus
    (0x45, 0x57), // kVK_ANSI_KeypadPlus
    (0x4C, 0x58), // kVK_ANSI_KeypadEnter
    (0x53, 0x59), // kVK_ANSI_Keypad1
    (0x54, 0x5A), // kVK_ANSI_Keypad2
    (0x55, 0x5B), // kVK_ANSI_Keypad3
    (0x56, 0x5C), // kVK_ANSI_Keypad4
    (0x57, 0x5D), // kVK_ANSI_Keypad5
    (0x58, 0x5E), // kVK_ANSI_Keypad6
    (0x59, 0x5F), // kVK_ANSI_Keypad7
    (0x5B, 0x60), // kVK_ANSI_Keypad8
    (0x5C, 0x61), // kVK_ANSI_Keypad9
    (0x52, 0x62), // kVK_ANSI_Keypad0
    (0x41, 0x63), // kVK_ANSI_KeypadDecimal
    // Modifier keys
    (0x38, 0xE1), // kVK_Shift -> Left Shift
    (0x3C, 0xE5), // kVK_RightShift
    (0x3B, 0xE0), // kVK_Control -> Left Control
    (0x3E, 0xE4), // kVK_RightControl
    (0x3A, 0xE2), // kVK_Option -> Left Alt
    (0x3D, 0xE6), // kVK_RightOption -> Right Alt
    (0x37, 0xE3), // kVK_Command -> Left GUI
    (0x36, 0xE7), // kVK_RightCommand -> Right GUI
];

#[cfg(test)]
mod tests {
    use super::*;

    // === VKey <-> HID roundtrip tests ===

    #[test]
    fn test_vkey_a_to_hid_and_back() {
        let hid = vkey_to_hid(0x41).unwrap(); // VK_A
        assert_eq!(hid, 0x04); // HID Usage: Keyboard a
        let vkey = hid_to_vkey(hid).unwrap();
        assert_eq!(vkey, 0x41);
    }

    #[test]
    fn test_vkey_enter_to_hid_and_back() {
        let hid = vkey_to_hid(0x0D).unwrap(); // VK_RETURN
        assert_eq!(hid, 0x28); // HID Usage: Keyboard Return
        let vkey = hid_to_vkey(hid).unwrap();
        assert_eq!(vkey, 0x0D);
    }

    #[test]
    fn test_vkey_space_to_hid_and_back() {
        let hid = vkey_to_hid(0x20).unwrap(); // VK_SPACE
        assert_eq!(hid, 0x2C);
        assert_eq!(hid_to_vkey(hid).unwrap(), 0x20);
    }

    #[test]
    fn test_vkey_f1_to_hid_and_back() {
        let hid = vkey_to_hid(0x70).unwrap(); // VK_F1
        assert_eq!(hid, 0x3A);
        assert_eq!(hid_to_vkey(hid).unwrap(), 0x70);
    }

    #[test]
    fn test_vkey_arrows_to_hid() {
        assert_eq!(vkey_to_hid(0x25).unwrap(), 0x50); // VK_LEFT
        assert_eq!(vkey_to_hid(0x26).unwrap(), 0x52); // VK_UP
        assert_eq!(vkey_to_hid(0x27).unwrap(), 0x4F); // VK_RIGHT
        assert_eq!(vkey_to_hid(0x28).unwrap(), 0x51); // VK_DOWN
    }

    #[test]
    fn test_vkey_modifiers_to_hid() {
        assert_eq!(vkey_to_hid(0xA0).unwrap(), 0xE1); // VK_LSHIFT
        assert_eq!(vkey_to_hid(0xA2).unwrap(), 0xE0); // VK_LCONTROL
        assert_eq!(vkey_to_hid(0xA4).unwrap(), 0xE2); // VK_LMENU (Alt)
        assert_eq!(vkey_to_hid(0x5B).unwrap(), 0xE3); // VK_LWIN
    }

    #[test]
    fn test_vkey_unknown_returns_none() {
        assert!(vkey_to_hid(0xFF).is_none());
    }

    #[test]
    fn test_generic_modifier_vkeys_not_in_table() {
        // Raw Input reports these generic VKeys for modifiers.
        // They must be resolved to left/right-specific VKeys BEFORE keymap lookup.
        assert!(
            vkey_to_hid(0x10).is_none(),
            "VK_SHIFT should not be in table"
        );
        assert!(
            vkey_to_hid(0x11).is_none(),
            "VK_CONTROL should not be in table"
        );
        assert!(
            vkey_to_hid(0x12).is_none(),
            "VK_MENU should not be in table"
        );
    }

    // === CGKeyCode <-> HID roundtrip tests ===

    #[test]
    fn test_cgkeycode_a_to_hid_and_back() {
        let hid = cgkeycode_to_hid(0x00).unwrap(); // kVK_ANSI_A
        assert_eq!(hid, 0x04);
        let code = hid_to_cgkeycode(hid).unwrap();
        assert_eq!(code, 0x00);
    }

    #[test]
    fn test_cgkeycode_return_to_hid_and_back() {
        let hid = cgkeycode_to_hid(0x24).unwrap(); // kVK_Return
        assert_eq!(hid, 0x28);
        assert_eq!(hid_to_cgkeycode(hid).unwrap(), 0x24);
    }

    #[test]
    fn test_cgkeycode_space_to_hid_and_back() {
        let hid = cgkeycode_to_hid(0x31).unwrap(); // kVK_Space
        assert_eq!(hid, 0x2C);
        assert_eq!(hid_to_cgkeycode(hid).unwrap(), 0x31);
    }

    #[test]
    fn test_cgkeycode_arrows_to_hid() {
        assert_eq!(cgkeycode_to_hid(0x7B).unwrap(), 0x50); // kVK_LeftArrow
        assert_eq!(cgkeycode_to_hid(0x7E).unwrap(), 0x52); // kVK_UpArrow
        assert_eq!(cgkeycode_to_hid(0x7C).unwrap(), 0x4F); // kVK_RightArrow
        assert_eq!(cgkeycode_to_hid(0x7D).unwrap(), 0x51); // kVK_DownArrow
    }

    #[test]
    fn test_cgkeycode_unknown_returns_none() {
        assert!(cgkeycode_to_hid(0xFE).is_none());
    }

    // === Cross-platform roundtrip: VKey -> HID -> CGKeyCode ===

    #[test]
    fn test_cross_platform_letter_a() {
        let hid = vkey_to_hid(0x41).unwrap(); // Windows VK_A -> HID
        let cg = hid_to_cgkeycode(hid).unwrap(); // HID -> macOS
        assert_eq!(cg, 0x00); // kVK_ANSI_A
    }

    #[test]
    fn test_cross_platform_letter_z() {
        let hid = vkey_to_hid(0x5A).unwrap(); // Windows VK_Z -> HID
        let cg = hid_to_cgkeycode(hid).unwrap(); // HID -> macOS
        assert_eq!(cg, 0x06); // kVK_ANSI_Z
    }

    #[test]
    fn test_cross_platform_enter() {
        let hid = vkey_to_hid(0x0D).unwrap(); // VK_RETURN -> HID
        let cg = hid_to_cgkeycode(hid).unwrap(); // HID -> macOS
        assert_eq!(cg, 0x24); // kVK_Return

        // Reverse: macOS -> Windows
        let hid2 = cgkeycode_to_hid(0x24).unwrap(); // kVK_Return -> HID
        let vk = hid_to_vkey(hid2).unwrap(); // HID -> Windows
        assert_eq!(vk, 0x0D); // VK_RETURN
    }

    #[test]
    fn test_all_letters_roundtrip() {
        // VK_A..VK_Z = 0x41..0x5A
        for vk in 0x41u16..=0x5A {
            let hid =
                vkey_to_hid(vk).unwrap_or_else(|| panic!("VKey 0x{vk:02X} should map to HID"));
            let cg = hid_to_cgkeycode(hid)
                .unwrap_or_else(|| panic!("HID 0x{hid:02X} should map to CGKeyCode"));
            let hid2 = cgkeycode_to_hid(cg)
                .unwrap_or_else(|| panic!("CGKeyCode 0x{cg:02X} should map back to HID"));
            assert_eq!(hid, hid2, "Roundtrip failed for VKey 0x{vk:02X}");
        }
    }

    #[test]
    fn test_all_digits_roundtrip() {
        // VK_0..VK_9 = 0x30..0x39
        for vk in 0x30u16..=0x39 {
            let hid =
                vkey_to_hid(vk).unwrap_or_else(|| panic!("VKey 0x{vk:02X} should map to HID"));
            let cg = hid_to_cgkeycode(hid)
                .unwrap_or_else(|| panic!("HID 0x{hid:02X} should map to CGKeyCode"));
            let hid2 = cgkeycode_to_hid(cg)
                .unwrap_or_else(|| panic!("CGKeyCode 0x{cg:02X} should map back to HID"));
            assert_eq!(hid, hid2, "Roundtrip failed for VKey 0x{vk:02X}");
        }
    }

    #[test]
    fn test_function_keys_roundtrip() {
        // VK_F1..VK_F12 = 0x70..0x7B
        for vk in 0x70u16..=0x7B {
            let hid =
                vkey_to_hid(vk).unwrap_or_else(|| panic!("VKey 0x{vk:02X} should map to HID"));
            let cg = hid_to_cgkeycode(hid)
                .unwrap_or_else(|| panic!("HID 0x{hid:02X} should map to CGKeyCode"));
            let hid2 = cgkeycode_to_hid(cg)
                .unwrap_or_else(|| panic!("CGKeyCode 0x{cg:02X} should map back to HID"));
            assert_eq!(hid, hid2, "Roundtrip failed for F key VKey 0x{vk:02X}");
        }
    }
}
