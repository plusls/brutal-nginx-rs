use core::{
    ffi::{c_char, c_void},
    ptr,
};
use std::ptr::NonNull;

use nginx_sys::{
    NGX_HTTP_MODULE, NGX_HTTP_SRV_CONF, NGX_HTTP_SRV_CONF_OFFSET, ngx_http_complex_value_t,
    ngx_http_module_t,
};
use ngx::{
    core::{NGX_CONF_ERROR, NGX_CONF_OK, Status},
    ffi::{NGX_CONF_TAKE1, ngx_command_t, ngx_conf_t, ngx_int_t, ngx_module_t, ngx_uint_t},
    http::{self, HttpModule as _, HttpModuleLocationConf, HttpPhase, HttpRequestHandler, Request},
    ngx_string,
};

use brutal_nginx_rs::{ModuleConfig, brutal_handler, extract_nginx_value, value::NginxBool};

struct Module;

impl http::HttpModule for Module {
    fn module() -> &'static ngx_module_t {
        unsafe { &*::core::ptr::addr_of!(ngx_http_brutal_module) }
    }

    unsafe extern "C" fn postconfiguration(cf: *mut ngx_conf_t) -> ngx_int_t {
        // SAFETY: this function is called with non-NULL cf always
        let cf = unsafe { &mut *cf };
        http::add_phase_handler::<BrutalSessionHandler>(cf)
            .map_or(Status::NGX_ERROR, |_| Status::NGX_OK)
            .into()
    }
}

unsafe impl HttpModuleLocationConf for Module {
    type LocationConf = ModuleConfig<ngx_http_complex_value_t>;
}

pub struct BrutalSessionHandler;
impl HttpRequestHandler for BrutalSessionHandler {
    const PHASE: HttpPhase = HttpPhase::Access;
    type Output = Status;

    fn handler(request: &mut Request) -> Self::Output {
        let brutual_params = Module::location_conf_mut(request)
            .expect("module config is none")
            .get_brutal_param(request);
        brutal_handler(request, brutual_params)
    }
}

static mut NGX_HTTP_BRUTAL_COMMANDS: [ngx_command_t; 4] = [
    ngx_command_t {
        name: ngx_string!("brutal"),
        type_: (NGX_HTTP_SRV_CONF | NGX_CONF_TAKE1) as ngx_uint_t,
        set: Some(ngx_http_brutal_set),
        conf: NGX_HTTP_SRV_CONF_OFFSET,
        offset: 0,
        post: ptr::null_mut(),
    },
    ngx_command_t {
        name: ngx_string!("brutal_rate"),
        type_: (NGX_HTTP_SRV_CONF | NGX_CONF_TAKE1) as ngx_uint_t,
        set: Some(ngx_http_brutal_rate_commands_set),
        conf: NGX_HTTP_SRV_CONF_OFFSET,
        offset: 0,
        post: ptr::null_mut(),
    },
    ngx_command_t {
        name: ngx_string!("brutal_cwnd_gain"),
        type_: (NGX_HTTP_SRV_CONF | NGX_CONF_TAKE1) as ngx_uint_t,
        set: Some(ngx_http_brutal_cwnd_gain_commands_set),
        conf: NGX_HTTP_SRV_CONF_OFFSET,
        offset: 0,
        post: ptr::null_mut(),
    },
    ngx_command_t::empty(),
];

static NGX_HTTP_BRUTAL_MODULE_CTX: ngx_http_module_t = ngx_http_module_t {
    preconfiguration: Some(Module::preconfiguration),
    postconfiguration: Some(Module::postconfiguration),
    create_main_conf: None,
    init_main_conf: None,
    create_srv_conf: None,
    merge_srv_conf: None,
    create_loc_conf: Some(Module::create_loc_conf),
    merge_loc_conf: Some(Module::merge_loc_conf),
};

// Generate the `ngx_modules` table with exported modules.
// This feature is required to build a 'cdylib' dynamic module outside of the NGINX buildsystem.
#[cfg(feature = "export-modules")]
ngx::ngx_modules!(ngx_http_brutal_module);

#[used]
#[allow(non_upper_case_globals)]
#[cfg_attr(not(feature = "export-modules"), unsafe(no_mangle))]
pub static mut ngx_http_brutal_module: ngx_module_t = ngx_module_t {
    ctx: &raw const NGX_HTTP_BRUTAL_MODULE_CTX as _,
    commands: unsafe { &raw mut NGX_HTTP_BRUTAL_COMMANDS[0] },
    type_: NGX_HTTP_MODULE as _,
    ..ngx_module_t::default()
};

extern "C" fn ngx_http_brutal_set(
    cf: *mut ngx_conf_t,
    cmd: *mut ngx_command_t,
    conf: *mut c_void,
) -> *mut c_char {
    let conf = unsafe { &mut *(conf as *mut ModuleConfig<ngx_http_complex_value_t>) };
    if let Some(v) = extract_nginx_value::<NginxBool, ngx_http_complex_value_t>(
        NonNull::new(cf).expect("cf not null"),
        NonNull::new(cmd).expect("cmd not null"),
    ) {
        conf.enable = Some(v);
        NGX_CONF_OK
    } else {
        NGX_CONF_ERROR
    }
}

extern "C" fn ngx_http_brutal_rate_commands_set(
    cf: *mut ngx_conf_t,
    cmd: *mut ngx_command_t,
    conf: *mut c_void,
) -> *mut c_char {
    let conf = unsafe { &mut *(conf as *mut ModuleConfig<ngx_http_complex_value_t>) };
    if let Some(v) = extract_nginx_value::<u64, ngx_http_complex_value_t>(
        NonNull::new(cf).expect("cf not null"),
        NonNull::new(cmd).expect("cmd not null"),
    ) {
        conf.rate = Some(v);
        NGX_CONF_OK
    } else {
        NGX_CONF_ERROR
    }
}

extern "C" fn ngx_http_brutal_cwnd_gain_commands_set(
    cf: *mut ngx_conf_t,
    cmd: *mut ngx_command_t,
    conf: *mut c_void,
) -> *mut c_char {
    let conf = unsafe { &mut *(conf as *mut ModuleConfig<ngx_http_complex_value_t>) };
    if let Some(v) = extract_nginx_value::<u32, ngx_http_complex_value_t>(
        NonNull::new(cf).expect("cf not null"),
        NonNull::new(cmd).expect("cmd not null"),
    ) {
        conf.cwnd_gain = Some(v);
        NGX_CONF_OK
    } else {
        NGX_CONF_ERROR
    }
}
