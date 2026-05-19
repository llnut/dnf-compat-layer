//! GeoIP Legacy compatible shared library.

#![allow(private_interfaces)]

use std::ffi::{CStr, c_char, c_int};
use std::fs;
use std::net::Ipv4Addr;

const GEOIP_COUNTRY_EDITION: u8 = 1;
// 0xFFFF00: segment base defined in the MaxMind GeoIP Legacy spec for Country edition.
const GEOIP_COUNTRY_BEGIN: u32 = 16776960;
const RECORD_LEN: usize = 3;

// ISO 3166-1 alpha-2 country codes in MaxMind GeoIP Legacy order.
const COUNTRY_CODES: [&[u8; 3]; 256] = [
    b"--\0", b"AP\0", b"EU\0", b"AD\0", b"AE\0", b"AF\0", b"AG\0", b"AI\0", b"AL\0", b"AM\0",
    b"CW\0", b"AO\0", b"AQ\0", b"AR\0", b"AS\0", b"AT\0", b"AU\0", b"AW\0", b"AZ\0", b"BA\0",
    b"BB\0", b"BD\0", b"BE\0", b"BF\0", b"BG\0", b"BH\0", b"BI\0", b"BJ\0", b"BM\0", b"BN\0",
    b"BO\0", b"BR\0", b"BS\0", b"BT\0", b"BV\0", b"BW\0", b"BY\0", b"BZ\0", b"CA\0", b"CC\0",
    b"CD\0", b"CF\0", b"CG\0", b"CH\0", b"CI\0", b"CK\0", b"CL\0", b"CM\0", b"CN\0", b"CO\0",
    b"CR\0", b"CU\0", b"CV\0", b"CX\0", b"CY\0", b"CZ\0", b"DE\0", b"DJ\0", b"DK\0", b"DM\0",
    b"DO\0", b"DZ\0", b"EC\0", b"EE\0", b"EG\0", b"EH\0", b"ER\0", b"ES\0", b"ET\0", b"FI\0",
    b"FJ\0", b"FK\0", b"FM\0", b"FO\0", b"FR\0", b"SX\0", b"GA\0", b"GB\0", b"GD\0", b"GE\0",
    b"GF\0", b"GH\0", b"GI\0", b"GL\0", b"GM\0", b"GN\0", b"GP\0", b"GQ\0", b"GR\0", b"GS\0",
    b"GT\0", b"GU\0", b"GW\0", b"GY\0", b"HK\0", b"HM\0", b"HN\0", b"HR\0", b"HT\0", b"HU\0",
    b"ID\0", b"IE\0", b"IL\0", b"IN\0", b"IO\0", b"IQ\0", b"IR\0", b"IS\0", b"IT\0", b"JM\0",
    b"JO\0", b"JP\0", b"KE\0", b"KG\0", b"KH\0", b"KI\0", b"KM\0", b"KN\0", b"KP\0", b"KR\0",
    b"KW\0", b"KY\0", b"KZ\0", b"LA\0", b"LB\0", b"LC\0", b"LI\0", b"LK\0", b"LR\0", b"LS\0",
    b"LT\0", b"LU\0", b"LV\0", b"LY\0", b"MA\0", b"MC\0", b"MD\0", b"MG\0", b"MH\0", b"MK\0",
    b"ML\0", b"MM\0", b"MN\0", b"MO\0", b"MP\0", b"MQ\0", b"MR\0", b"MS\0", b"MT\0", b"MU\0",
    b"MV\0", b"MW\0", b"MX\0", b"MY\0", b"MZ\0", b"NA\0", b"NC\0", b"NE\0", b"NF\0", b"NG\0",
    b"NI\0", b"NL\0", b"NO\0", b"NP\0", b"NR\0", b"NU\0", b"NZ\0", b"OM\0", b"PA\0", b"PE\0",
    b"PF\0", b"PG\0", b"PH\0", b"PK\0", b"PL\0", b"PM\0", b"PN\0", b"PR\0", b"PS\0", b"PT\0",
    b"PW\0", b"PY\0", b"QA\0", b"RE\0", b"RO\0", b"RU\0", b"RW\0", b"SA\0", b"SB\0", b"SC\0",
    b"SD\0", b"SE\0", b"SG\0", b"SH\0", b"SI\0", b"SJ\0", b"SK\0", b"SL\0", b"SM\0", b"SN\0",
    b"SO\0", b"SR\0", b"ST\0", b"SV\0", b"SY\0", b"SZ\0", b"TC\0", b"TD\0", b"TF\0", b"TG\0",
    b"TH\0", b"TJ\0", b"TK\0", b"TM\0", b"TN\0", b"TO\0", b"TL\0", b"TR\0", b"TT\0", b"TV\0",
    b"TW\0", b"TZ\0", b"UA\0", b"UG\0", b"UM\0", b"US\0", b"UY\0", b"UZ\0", b"VA\0", b"VC\0",
    b"VE\0", b"VG\0", b"VI\0", b"VN\0", b"VU\0", b"WF\0", b"WS\0", b"YE\0", b"YT\0", b"RS\0",
    b"ZA\0", b"ZM\0", b"ME\0", b"ZW\0", b"A1\0", b"A2\0", b"O1\0", b"AX\0", b"GG\0", b"IM\0",
    b"JE\0", b"BL\0", b"MF\0", b"BQ\0", b"SS\0", b"--\0", // index 255: out of valid range
];

const GEOIP_DAT_PATHS: [&str; 4] = [
    "/usr/local/share/GeoIP/GeoIP.dat",
    "/usr/share/GeoIP/GeoIP.dat",
    "/var/lib/GeoIP/GeoIP.dat",
    "/data/GeoIP/GeoIP.dat",
];

struct GeoIpDb {
    data: Vec<u8>,
    segments: u32,
}

fn parse_db(data: Vec<u8>) -> Option<GeoIpDb> {
    if data.len() < 10 {
        return None;
    }

    // Validate edition type by scanning for the 3-byte marker 0xFF 0xFF 0xFF
    // near the end of the file. The byte after the marker is the edition type.
    // Only Country edition with type value 1 is supported.
    let len = data.len();
    let search_start = len.saturating_sub(100);
    let mut found_country = false;

    // Loop bound len-3 ensures i+3 < len, so the edition byte is always accessible.
    for i in search_start..len.saturating_sub(3) {
        if data[i] == 0xFF && data[i + 1] == 0xFF && data[i + 2] == 0xFF {
            found_country = data[i + 3] == GEOIP_COUNTRY_EDITION;
            break;
        }
    }

    // Reject non-Country editions.
    if !found_country {
        return None;
    }

    Some(GeoIpDb {
        data,
        segments: GEOIP_COUNTRY_BEGIN,
    })
}

impl GeoIpDb {
    fn open_default() -> Option<Box<Self>> {
        for path in &GEOIP_DAT_PATHS {
            if let Ok(data) = fs::read(path)
                && let Some(db) = parse_db(data)
            {
                return Some(Box::new(db));
            }
        }
        None
    }

    fn lookup_country_id(&self, ip: u32) -> Option<usize> {
        let mut offset = 0u32;

        for depth in (0..32).rev() {
            let bit = ((ip >> depth) & 1) as usize;
            let record_offset = (offset as usize) * 2 * RECORD_LEN + bit * RECORD_LEN;

            if record_offset + RECORD_LEN > self.data.len() {
                return None;
            }

            let mut val = 0u32;
            for j in 0..RECORD_LEN {
                val |= (self.data[record_offset + j] as u32) << (j * 8);
            }

            if val >= self.segments {
                return Some((val - self.segments) as usize);
            }
            offset = val;
        }
        None
    }

    /// Returns a pointer to a null-terminated static country code string, or null.
    fn country_code_by_addr(&self, addr: &str) -> *const c_char {
        let ip: Ipv4Addr = match addr.parse() {
            Ok(ip) => ip,
            Err(_) => return std::ptr::null(),
        };
        let ip_num = u32::from(ip);
        let country_id = match self.lookup_country_id(ip_num) {
            Some(id) => id,
            None => return std::ptr::null(),
        };

        if country_id < COUNTRY_CODES.len() && &COUNTRY_CODES[country_id][..2] != b"--" {
            // Return pointer into static array — valid for the lifetime of the program.
            COUNTRY_CODES[country_id].as_ptr() as *const c_char
        } else {
            std::ptr::null()
        }
    }
}

// C ABI exports

/// Opens the GeoIP database using the default search paths.
/// The `db_type` argument is accepted for API compatibility but is not used.
/// Returns a handle on success, or NULL if no usable database is found.
///
/// # Safety
/// The returned pointer must be freed with `GeoIP_delete`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn GeoIP_new(_db_type: c_int) -> *mut GeoIpDb {
    match GeoIpDb::open_default() {
        Some(db) => Box::into_raw(db),
        None => std::ptr::null_mut(),
    }
}

/// Opens a GeoIP database from the given file path.
/// If `path` is NULL, the default search paths are used. Open flags are accepted
/// for API compatibility but are not used.
/// Returns a handle on success, or NULL on failure.
///
/// # Safety
/// - `path` must be NULL or a valid pointer to a null-terminated C string.
/// - The returned pointer must be freed with `GeoIP_delete`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn GeoIP_open(path: *const c_char, _flags: c_int) -> *mut GeoIpDb {
    if path.is_null() {
        return match GeoIpDb::open_default() {
            Some(db) => Box::into_raw(db),
            None => std::ptr::null_mut(),
        };
    }

    let path_str = match unsafe { CStr::from_ptr(path) }.to_str() {
        Ok(s) => s,
        Err(_) => return std::ptr::null_mut(),
    };

    match fs::read(path_str) {
        Ok(data) => match parse_db(data) {
            Some(db) => Box::into_raw(Box::new(db)),
            None => std::ptr::null_mut(),
        },
        Err(_) => std::ptr::null_mut(),
    }
}

/// Returns a static null-terminated country code string such as "CN" or "US", or NULL.
/// The pointer is valid for the lifetime of the program. No deallocation is needed.
///
/// # Safety
/// - `gi` must be NULL or a valid pointer returned by `GeoIP_new` / `GeoIP_open`.
/// - `addr` must be NULL or a valid pointer to a null-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn GeoIP_country_code_by_addr(
    gi: *mut GeoIpDb,
    addr: *const c_char,
) -> *const c_char {
    if gi.is_null() || addr.is_null() {
        return std::ptr::null();
    }
    let db = unsafe { &*gi };
    let addr_str = match unsafe { CStr::from_ptr(addr) }.to_str() {
        Ok(s) => s,
        Err(_) => return std::ptr::null(),
    };
    db.country_code_by_addr(addr_str)
}

/// Closes and frees the database handle. Passing NULL is a no-op.
///
/// # Safety
/// `gi` must be NULL or a valid pointer returned by `GeoIP_new` / `GeoIP_open`.
/// After this call, `gi` must not be used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn GeoIP_delete(gi: *mut GeoIpDb) {
    if !gi.is_null() {
        let _ = unsafe { Box::from_raw(gi) };
    }
}

/// Only IPv4 address strings are accepted.
/// DNS hostnames are NOT resolved, passing a hostname returns NULL.
///
/// # Safety
/// - `gi` must be NULL or a valid pointer returned by `GeoIP_new` / `GeoIP_open`.
/// - `name` must be NULL or a valid pointer to a null-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn GeoIP_country_code_by_name(
    gi: *mut GeoIpDb,
    name: *const c_char,
) -> *const c_char {
    unsafe { GeoIP_country_code_by_addr(gi, name) }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Build a minimal .dat buffer containing the Country edition marker.
    fn make_country_dat() -> Vec<u8> {
        let mut data = vec![0u8; 16];
        data.extend_from_slice(&[0xFF, 0xFF, 0xFF, GEOIP_COUNTRY_EDITION]);
        data
    }

    #[test]
    fn parse_rejects_too_short() {
        assert!(parse_db(vec![0u8; 9]).is_none());
    }

    #[test]
    fn parse_rejects_missing_marker() {
        assert!(parse_db(vec![0u8; 20]).is_none());
    }

    #[test]
    fn parse_rejects_wrong_edition() {
        let mut data = vec![0u8; 16];
        data.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0x02]); // edition 2 = City
        assert!(parse_db(data).is_none());
    }

    #[test]
    fn parse_accepts_country_edition() {
        assert!(parse_db(make_country_dat()).is_some());
    }

    #[test]
    fn lookup_returns_none_on_empty_data() {
        let db = GeoIpDb {
            data: vec![0u8; 1],
            segments: GEOIP_COUNTRY_BEGIN,
        };
        assert!(db.lookup_country_id(0).is_none());
    }

    #[test]
    fn lookup_returns_country_id() {
        // Single-node trie: both branches resolve immediately.
        // Record value 0xFFFF01 in little-endian = GEOIP_COUNTRY_BEGIN + 1 = country index 1.
        let mut data = vec![0u8; 6];
        data[0] = 0x01;
        data[1] = 0xFF;
        data[2] = 0xFF; // left branch
        data[3] = 0x01;
        data[4] = 0xFF;
        data[5] = 0xFF; // right branch
        let db = GeoIpDb {
            data,
            segments: GEOIP_COUNTRY_BEGIN,
        };
        assert_eq!(db.lookup_country_id(0x00000000), Some(1));
        assert_eq!(db.lookup_country_id(0x80000000), Some(1));
    }
}
