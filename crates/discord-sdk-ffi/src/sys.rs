// SPDX-License-Identifier: AGPL-3.0-only
//! Raw declarations for the small part of the Discord Social SDK C API
//! (`cdiscord.h`, SDK 1.10) that RemoteMusicRP uses. Nothing here is
//! linked at build time: every symbol is resolved from the user-supplied
//! library at runtime by [`Api::load`].
//!
//! Layout notes (from `cdiscord.h`):
//! - every SDK object is `struct { void* opaque; }`;
//! - `Discord_String` is `{ uint8_t* ptr; size_t size; }` (not NUL-terminated);
//! - enums are plain C `int`s (each has a `_forceint = 0x7FFFFFFF` member);
//! - strings handed to callbacks are owned by the receiver and must be
//!   released with `Discord_Free`; input strings are copied by the SDK.

use std::ffi::c_void;

use libloading::Library;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DString {
    pub ptr: *mut u8,
    pub size: usize,
}

impl DString {
    /// An empty out-parameter for the SDK to fill.
    pub fn empty() -> Self {
        Self {
            ptr: std::ptr::null_mut(),
            size: 0,
        }
    }

    /// Borrow `s` for one SDK call (the SDK copies input strings).
    pub fn borrow(s: &str) -> Self {
        Self {
            ptr: s.as_ptr().cast_mut(),
            size: s.len(),
        }
    }
}

/// Every SDK object handle: `struct { void* opaque; }`.
#[repr(C)]
#[derive(Debug)]
pub struct Handle {
    pub opaque: *mut c_void,
}

impl Handle {
    pub fn null() -> Self {
        Self {
            opaque: std::ptr::null_mut(),
        }
    }
}

pub type FreeFn = unsafe extern "C" fn(*mut c_void);
pub type TokenCb = unsafe extern "C" fn(
    result: *mut Handle,
    access: DString,
    refresh: DString,
    token_type: i32,
    expires_in: i32,
    scopes: DString,
    user: *mut c_void,
);
pub type ResultCb = unsafe extern "C" fn(result: *mut Handle, user: *mut c_void);
pub type StatusCb = unsafe extern "C" fn(status: i32, error: i32, detail: i32, user: *mut c_void);
pub type LogCb = unsafe extern "C" fn(message: DString, severity: i32, user: *mut c_void);

pub const TOKEN_TYPE_BEARER: i32 = 1;
pub const LOG_ERROR: i32 = 4;

macro_rules! api {
    ($( $field:ident : $sym:literal => fn($($arg:ty),*) $(-> $ret:ty)? ;)*) => {
        /// Resolved SDK entry points.
        #[derive(Debug, Clone, Copy)]
        pub struct Api {
            $( pub $field: unsafe extern "C" fn($($arg),*) $(-> $ret)?, )*
        }

        impl Api {
            /// Resolve every symbol from `lib`.
            ///
            /// # Safety
            /// `lib` must be a genuine Discord Social SDK library whose C API
            /// matches the declarations above, and must outlive the returned
            /// pointers (the caller keeps it loaded for the process lifetime).
            pub unsafe fn load(lib: &Library) -> Result<Self, String> {
                Ok(Self {
                    $( $field: {
                        // SAFETY: the symbol type matches the C declaration in
                        // cdiscord.h (caller's contract); we copy the raw fn
                        // pointer out while `lib` stays loaded forever.
                        let s = unsafe {
                            lib.get::<unsafe extern "C" fn($($arg),*) $(-> $ret)?>($sym.as_bytes())
                        }
                        .map_err(|_| format!("the library has no {} (wrong file or SDK version?)", $sym))?;
                        *s
                    }, )*
                })
            }
        }
    };
}

api! {
    free: "Discord_Free" => fn(*mut c_void);
    run_callbacks: "Discord_RunCallbacks" => fn();

    client_init: "Discord_Client_Init" => fn(*mut Handle);
    client_drop: "Discord_Client_Drop" => fn(*mut Handle);
    client_add_log_callback: "Discord_Client_AddLogCallback"
        => fn(*mut Handle, LogCb, FreeFn, *mut c_void, i32);
    client_set_status_changed_callback: "Discord_Client_SetStatusChangedCallback"
        => fn(*mut Handle, StatusCb, FreeFn, *mut c_void);
    client_get_token: "Discord_Client_GetToken"
        => fn(*mut Handle, u64, DString, DString, DString, TokenCb, FreeFn, *mut c_void);
    client_refresh_token: "Discord_Client_RefreshToken"
        => fn(*mut Handle, u64, DString, TokenCb, FreeFn, *mut c_void);
    client_revoke_token: "Discord_Client_RevokeToken"
        => fn(*mut Handle, u64, DString, ResultCb, FreeFn, *mut c_void);
    client_update_token: "Discord_Client_UpdateToken"
        => fn(*mut Handle, i32, DString, ResultCb, FreeFn, *mut c_void);
    client_connect: "Discord_Client_Connect" => fn(*mut Handle);
    client_disconnect: "Discord_Client_Disconnect" => fn(*mut Handle);
    client_update_rich_presence: "Discord_Client_UpdateRichPresence"
        => fn(*mut Handle, *mut Handle, ResultCb, FreeFn, *mut c_void);
    client_clear_rich_presence: "Discord_Client_ClearRichPresence" => fn(*mut Handle);
    client_get_current_user_v2: "Discord_Client_GetCurrentUserV2"
        => fn(*mut Handle, *mut Handle) -> bool;

    client_set_online_status: "Discord_Client_SetOnlineStatus"
        => fn(*mut Handle, i32, ResultCb, FreeFn, *mut c_void);

    user_drop: "Discord_UserHandle_Drop" => fn(*mut Handle);
    user_status: "Discord_UserHandle_Status" => fn(*mut Handle) -> i32;
    user_id: "Discord_UserHandle_Id" => fn(*mut Handle) -> u64;
    user_username: "Discord_UserHandle_Username" => fn(*mut Handle, *mut DString);
    user_global_name: "Discord_UserHandle_GlobalName" => fn(*mut Handle, *mut DString) -> bool;
    user_avatar: "Discord_UserHandle_Avatar" => fn(*mut Handle, *mut DString) -> bool;

    result_successful: "Discord_ClientResult_Successful" => fn(*mut Handle) -> bool;
    result_error: "Discord_ClientResult_Error" => fn(*mut Handle, *mut DString);
    result_drop: "Discord_ClientResult_Drop" => fn(*mut Handle);

    activity_init: "Discord_Activity_Init" => fn(*mut Handle);
    activity_drop: "Discord_Activity_Drop" => fn(*mut Handle);
    activity_set_type: "Discord_Activity_SetType" => fn(*mut Handle, i32);
    activity_set_name: "Discord_Activity_SetName" => fn(*mut Handle, DString);
    activity_set_details: "Discord_Activity_SetDetails" => fn(*mut Handle, *mut DString);
    activity_set_details_url: "Discord_Activity_SetDetailsUrl" => fn(*mut Handle, *mut DString);
    activity_set_state: "Discord_Activity_SetState" => fn(*mut Handle, *mut DString);
    activity_set_state_url: "Discord_Activity_SetStateUrl" => fn(*mut Handle, *mut DString);
    activity_set_status_display_type: "Discord_Activity_SetStatusDisplayType"
        => fn(*mut Handle, *mut i32);
    activity_set_assets: "Discord_Activity_SetAssets" => fn(*mut Handle, *mut Handle);
    activity_set_timestamps: "Discord_Activity_SetTimestamps" => fn(*mut Handle, *mut Handle);
    activity_add_button: "Discord_Activity_AddButton" => fn(*mut Handle, *const Handle);

    assets_init: "Discord_ActivityAssets_Init" => fn(*mut Handle);
    assets_drop: "Discord_ActivityAssets_Drop" => fn(*mut Handle);
    assets_set_large_image: "Discord_ActivityAssets_SetLargeImage" => fn(*mut Handle, *mut DString);
    assets_set_large_text: "Discord_ActivityAssets_SetLargeText" => fn(*mut Handle, *mut DString);
    assets_set_large_url: "Discord_ActivityAssets_SetLargeUrl" => fn(*mut Handle, *mut DString);
    assets_set_small_image: "Discord_ActivityAssets_SetSmallImage" => fn(*mut Handle, *mut DString);
    assets_set_small_text: "Discord_ActivityAssets_SetSmallText" => fn(*mut Handle, *mut DString);

    timestamps_init: "Discord_ActivityTimestamps_Init" => fn(*mut Handle);
    timestamps_drop: "Discord_ActivityTimestamps_Drop" => fn(*mut Handle);
    timestamps_set_start: "Discord_ActivityTimestamps_SetStart" => fn(*mut Handle, u64);
    timestamps_set_end: "Discord_ActivityTimestamps_SetEnd" => fn(*mut Handle, u64);

    button_init: "Discord_ActivityButton_Init" => fn(*mut Handle);
    button_drop: "Discord_ActivityButton_Drop" => fn(*mut Handle);
    button_set_label: "Discord_ActivityButton_SetLabel" => fn(*mut Handle, DString);
    button_set_url: "Discord_ActivityButton_SetUrl" => fn(*mut Handle, DString);
}
