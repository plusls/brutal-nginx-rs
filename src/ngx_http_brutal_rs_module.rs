use core::{
    ffi::{c_char, c_void},
    ptr,
};
use std::ptr::NonNull;

use nginx_sys::ngx_stream_complex_value_t;
use ngx::{
    core::{NGX_CONF_ERROR, NGX_CONF_OK, Status},
    ffi::{
        NGX_CONF_TAKE1, NGX_STREAM_MODULE, NGX_STREAM_SRV_CONF, NGX_STREAM_SRV_CONF_OFFSET,
        ngx_command_t, ngx_conf_t, ngx_int_t, ngx_module_t, ngx_stream_module_t, ngx_uint_t,
    },
    ngx_string,
};

use brutal_nginx_rs::{
    ModuleConfig, brutal_handler, extract_nginx_value,
    stream::{
        self, Session, StreamModule, StreamModuleServerConf, StreamPhase, StreamSessionHandler,
    },
    value::NginxBool,
};

struct Module;

impl stream::StreamModule for Module {
    fn module() -> &'static ngx_module_t {
        unsafe { &*::core::ptr::addr_of!(ngx_stream_brutal_module) }
    }

    unsafe extern "C" fn postconfiguration(cf: *mut ngx_conf_t) -> ngx_int_t {
        // SAFETY: this function is called with non-NULL cf always
        let cf = unsafe { &mut *cf };
        stream::add_phase_handler::<BrutalSessionHandler>(cf)
            .map_or(Status::NGX_ERROR, |_| Status::NGX_OK)
            .into()
    }
}

unsafe impl StreamModuleServerConf for Module {
    type ServerConf = ModuleConfig<ngx_stream_complex_value_t>;
}

pub struct BrutalSessionHandler;
impl StreamSessionHandler for BrutalSessionHandler {
    const PHASE: StreamPhase = StreamPhase::PostAccept;
    type Output = Status;

    fn handler(session: &mut Session) -> Self::Output {
        let brutual_params = Module::server_conf_mut(session)
            .expect("module config is none")
            .get_brutal_param(session);
        brutal_handler(session, brutual_params)
    }
}

static mut NGX_STREAM_BRUTAL_COMMANDS: [ngx_command_t; 4] = [
    ngx_command_t {
        name: ngx_string!("brutal"),
        type_: (NGX_STREAM_SRV_CONF | NGX_CONF_TAKE1) as ngx_uint_t,
        set: Some(ngx_stream_brutal_set),
        conf: NGX_STREAM_SRV_CONF_OFFSET,
        offset: 0,
        post: ptr::null_mut(),
    },
    ngx_command_t {
        name: ngx_string!("brutal_rate"),
        type_: (NGX_STREAM_SRV_CONF | NGX_CONF_TAKE1) as ngx_uint_t,
        set: Some(ngx_stream_brutal_rate_commands_set),
        conf: NGX_STREAM_SRV_CONF_OFFSET,
        offset: 0,
        post: ptr::null_mut(),
    },
    ngx_command_t {
        name: ngx_string!("brutal_cwnd_gain"),
        type_: (NGX_STREAM_SRV_CONF | NGX_CONF_TAKE1) as ngx_uint_t,
        set: Some(ngx_stream_brutal_cwnd_gain_commands_set),
        conf: NGX_STREAM_SRV_CONF_OFFSET,
        offset: 0,
        post: ptr::null_mut(),
    },
    ngx_command_t::empty(),
];

static NGX_STREAM_BRUTAL_MODULE_CTX: ngx_stream_module_t = ngx_stream_module_t {
    preconfiguration: Some(Module::preconfiguration),
    postconfiguration: Some(Module::postconfiguration),
    create_main_conf: None,
    init_main_conf: None,
    create_srv_conf: Some(Module::create_srv_conf),
    merge_srv_conf: Some(Module::merge_srv_conf),
};

// Generate the `ngx_modules` table with exported modules.
// This feature is required to build a 'cdylib' dynamic module outside of the NGINX buildsystem.
#[cfg(feature = "export-modules")]
ngx::ngx_modules!(ngx_stream_brutal_module);

#[used]
#[allow(non_upper_case_globals)]
#[cfg_attr(not(feature = "export-modules"), unsafe(no_mangle))]
pub static mut ngx_stream_brutal_module: ngx_module_t = ngx_module_t {
    ctx: &raw const NGX_STREAM_BRUTAL_MODULE_CTX as _,
    commands: unsafe { &raw mut NGX_STREAM_BRUTAL_COMMANDS[0] },
    type_: NGX_STREAM_MODULE as _,
    ..ngx_module_t::default()
};

extern "C" fn ngx_stream_brutal_set(
    cf: *mut ngx_conf_t,
    cmd: *mut ngx_command_t,
    conf: *mut c_void,
) -> *mut c_char {
    let conf = unsafe { &mut *(conf as *mut ModuleConfig<ngx_stream_complex_value_t>) };
    if let Some(v) = extract_nginx_value::<NginxBool, ngx_stream_complex_value_t>(
        NonNull::new(cf).expect("cf not null"),
        NonNull::new(cmd).expect("cmd not null"),
    ) {
        conf.enable = Some(v);
        NGX_CONF_OK
    } else {
        NGX_CONF_ERROR
    }
}

extern "C" fn ngx_stream_brutal_rate_commands_set(
    cf: *mut ngx_conf_t,
    cmd: *mut ngx_command_t,
    conf: *mut c_void,
) -> *mut c_char {
    let conf = unsafe { &mut *(conf as *mut ModuleConfig<ngx_stream_complex_value_t>) };
    if let Some(v) = extract_nginx_value::<u64, ngx_stream_complex_value_t>(
        NonNull::new(cf).expect("cf not null"),
        NonNull::new(cmd).expect("cmd not null"),
    ) {
        conf.rate = Some(v);
        NGX_CONF_OK
    } else {
        NGX_CONF_ERROR
    }
}

extern "C" fn ngx_stream_brutal_cwnd_gain_commands_set(
    cf: *mut ngx_conf_t,
    cmd: *mut ngx_command_t,
    conf: *mut c_void,
) -> *mut c_char {
    let conf = unsafe { &mut *(conf as *mut ModuleConfig<ngx_stream_complex_value_t>) };
    if let Some(v) = extract_nginx_value::<u32, ngx_stream_complex_value_t>(
        NonNull::new(cf).expect("cf not null"),
        NonNull::new(cmd).expect("cmd not null"),
    ) {
        conf.cwnd_gain = Some(v);
        NGX_CONF_OK
    } else {
        NGX_CONF_ERROR
    }
}
