use core::{
    ffi::{c_char, c_void},
    ptr,
};
use std::{alloc::Layout, ptr::NonNull, str::FromStr};

use nginx_sys::{
    __socket_type_SOCK_STREAM, AF_UNIX, IPPROTO_TCP, NGX_LOG_ERR, NGX_OK, TCP_CONGESTION,
    ngx_stream_compile_complex_value, ngx_stream_compile_complex_value_t,
    ngx_stream_complex_value_t, setsockopt,
};
use ngx::{
    allocator::Allocator,
    core::{NGX_CONF_ERROR, NGX_CONF_OK, Pool, Status},
    ffi::{
        NGX_CONF_TAKE1, NGX_LOG_EMERG, NGX_STREAM_MODULE, NGX_STREAM_SRV_CONF,
        NGX_STREAM_SRV_CONF_OFFSET, ngx_command_t, ngx_conf_t, ngx_int_t, ngx_module_t, ngx_str_t,
        ngx_stream_module_t, ngx_uint_t,
    },
    http::{Merge, MergeConfigError},
    log::DebugMask,
    ngx_conf_log_error, ngx_log_debug, ngx_log_error, ngx_string,
};

use crate::{
    stream::{Session, StreamModule, StreamModuleServerConf, StreamPhase, StreamSessionHandler},
    value::{NginxBool, NginxStreamComplexValue, NginxStreamValue},
};

mod value;

#[cfg(ngx_feature = "stream")]
pub mod stream;
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

#[derive(Debug, Default)]
struct ModuleConfig {
    enable: Option<NginxStreamValue<NginxBool>>,
    rate: Option<NginxStreamValue<u64>>,
    cwnd_gain: Option<NginxStreamValue<u32>>,
}

impl ModuleConfig {
    fn get_brutal_param(&mut self, session: &mut Session) -> Option<BrutalParams> {
        if let Some(enable) = &mut self.enable {
            match enable.extract(Some(session)) {
                Ok(enable) => {
                    if !enable.as_bool() {
                        return None;
                    }
                }
                Err(err) => {
                    ngx_log_error!(NGX_LOG_EMERG, session.log(), "{err}",);
                }
            }
        } else {
            return None;
        }

        if let Some(rate) = &mut self.rate {
            let mut cwnd_gain_default = NginxStreamValue::Static(15);
            let cwnd_gain = self.cwnd_gain.as_mut().unwrap_or(&mut cwnd_gain_default);

            let rate = match rate.extract(Some(session)) {
                Ok(rate) => rate,
                Err(err) => {
                    ngx_log_error!(NGX_LOG_EMERG, session.log(), "{err}",);
                    return None;
                }
            };

            let cwnd_gain = match cwnd_gain.extract(Some(session)) {
                Ok(cwnd_gain) => cwnd_gain,
                Err(err) => {
                    ngx_log_error!(NGX_LOG_EMERG, session.log(), "{err}",);
                    return None;
                }
            };

            Some(BrutalParams { rate, cwnd_gain })
        } else {
            None
        }
    }
}

#[repr(C, packed)]
// TCP_BRUTAL_PARAMS expects the packed 12-byte kernel ABI: u64 rate + u32 cwnd_gain.
struct BrutalParams {
    rate: u64,
    cwnd_gain: u32,
}

unsafe impl StreamModuleServerConf for Module {
    type ServerConf = ModuleConfig;
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

impl Merge for ModuleConfig {
    fn merge(&mut self, prev: &ModuleConfig) -> Result<(), MergeConfigError> {
        if self.enable.is_none() {
            self.enable = prev.enable.clone();
        }
        if self.rate.is_none() {
            self.rate = prev.rate.clone();
        };
        if self.cwnd_gain.is_none() {
            self.cwnd_gain = prev.cwnd_gain.clone();
        };
        Ok(())
    }
}

struct BrutalSessionHandler;

impl StreamSessionHandler for BrutalSessionHandler {
    const PHASE: StreamPhase = StreamPhase::PostAccept;
    type Output = Status;

    fn handler(session: &mut Session) -> Self::Output {
        let brutal_param = Module::server_conf_mut(session)
            .expect("module config is none")
            .get_brutal_param(session);
        let enable = brutal_param.is_some();
        let connection = unsafe {
            session
                .connection()
                .as_ref()
                .expect("connection always not null")
        };
        let should_apply_brutal = connection.type_ as u32 == __socket_type_SOCK_STREAM
            && unsafe {
                connection
                    .listening
                    .as_ref()
                    .and_then(|listening| listening.sockaddr.as_ref())
                    .is_some_and(|sockaddr| sockaddr.sa_family as u32 != AF_UNIX)
            };

        if let Some(brutal_param) = brutal_param
            && should_apply_brutal
        {
            let fd = connection.fd;

            let log = session.log();
            ngx_log_debug!(mask: DebugMask::Stream, log, "brutal module enabled: {enable}");

            let algo = b"brutal\0";

            let set_sock_opt_ret = unsafe {
                setsockopt(
                    fd,
                    IPPROTO_TCP as i32,
                    TCP_CONGESTION as i32,
                    algo.as_ptr().cast(),
                    algo.len() as u32,
                )
            };
            if set_sock_opt_ret != 0 {
                ngx_log_error!(
                    NGX_LOG_ERR,
                    log,
                    "tcp brutal TCP_CONGESTION 1 error: {set_sock_opt_ret}"
                );
                return Status::NGX_DECLINED;
            }

            const TCP_BRUTAL_PARAMS: u32 = 23301;
            let set_sock_opt_ret = unsafe {
                setsockopt(
                    fd,
                    IPPROTO_TCP as i32,
                    TCP_BRUTAL_PARAMS as i32,
                    (&brutal_param as *const BrutalParams).cast(),
                    std::mem::size_of::<BrutalParams>() as u32,
                )
            };
            if set_sock_opt_ret != 0 {
                ngx_log_error!(
                    NGX_LOG_ERR,
                    log,
                    "tcp brutal TCP_CONGESTION 2 error: {set_sock_opt_ret}"
                );
                return Status::NGX_DECLINED;
            }
            Status::NGX_OK
        } else {
            Status::NGX_DECLINED
        }
    }
}

fn extract_nginx_stream_value<T: FromStr>(
    mut cf: NonNull<ngx_conf_t>,
    cmd: *mut ngx_command_t,
) -> Option<NginxStreamValue<T>> {
    unsafe {
        let args: &mut [ngx_str_t] = (*cf.as_ref().args).as_slice_mut();
        let cmd_name = (*cmd).name;
        let cmd_name = if let Ok(cmd_name) = cmd_name.to_str() {
            cmd_name
        } else {
            ngx_conf_log_error!(
                NGX_LOG_EMERG,
                cf.as_mut(),
                "cmd name {} not utf-8 encoded",
                cmd_name
            );
            return None;
        };

        let pool = Pool::from_ngx_pool(cf.as_ref().pool);
        let layout = Layout::new::<ngx_stream_complex_value_t>();
        let cv = pool.allocate(std::mem::zeroed::<ngx_stream_complex_value_t>());
        let mut cv = match NonNull::new(cv) {
            Some(cv) => cv,
            None => {
                ngx_conf_log_error!(
                    NGX_LOG_EMERG,
                    cf.as_mut(),
                    "failed to allocate ngx_stream_complex_value_t for `{cmd_name}`"
                );
                return None;
            }
        };

        let mut ccv: ngx_stream_compile_complex_value_t = std::mem::zeroed();
        ccv.cf = cf.as_mut();
        ccv.value = &mut args[1] as _;
        ccv.complex_value = cv.as_mut();

        if ngx_stream_compile_complex_value(&mut ccv as _) != NGX_OK as ngx_int_t {
            ngx_conf_log_error!(
                NGX_LOG_EMERG,
                cf.as_mut(),
                "`{cmd_name}` argument ngx_stream_compile_complex_value failed."
            );
            return None;
        }

        let val = if cv.as_ref().lengths.is_null() {
            // 静态值
            pool.deallocate(cv.cast::<u8>(), layout);
            let val = match args[1].to_str() {
                Ok(s) => s,
                Err(_) => {
                    ngx_conf_log_error!(
                        NGX_LOG_EMERG,
                        cf.as_mut(),
                        "`{cmd_name}` argument is not utf-8 encoded"
                    );
                    return None;
                }
            };

            let n = match val.parse::<T>() {
                Ok(n) => n,
                _ => {
                    ngx_conf_log_error!(NGX_LOG_EMERG, cf.as_mut(), "invalid {cmd_name}",);
                    return None;
                }
            };
            NginxStreamValue::Static(n)
        } else {
            let conf_file = cf.as_ref().conf_file;
            let (file_name, line) = if let Some(conf_file) = NonNull::new(conf_file) {
                let conf_file = conf_file.as_ref();
                let file_name = if let Ok(file_name) = conf_file.file.name.to_str() {
                    file_name.to_string()
                } else {
                    ngx_conf_log_error!(
                        NGX_LOG_EMERG,
                        cf.as_mut(),
                        "invalid config file name {}",
                        conf_file.file.name
                    );
                    return None;
                };
                (file_name, conf_file.line as u32)
            } else {
                ngx_conf_log_error!(NGX_LOG_EMERG, cf.as_mut(), "conf_file name is null",);
                return None;
            };
            // complex value，里面有变量
            NginxStreamValue::Complex(NginxStreamComplexValue {
                cv,
                cmd_name: cmd_name.to_string(),
                file_name,
                line,
            })
        };
        Some(val)
    }
}

extern "C" fn ngx_stream_brutal_set(
    cf: *mut ngx_conf_t,
    cmd: *mut ngx_command_t,
    conf: *mut c_void,
) -> *mut c_char {
    let conf = unsafe { &mut *(conf as *mut ModuleConfig) };
    if let Some(v) =
        extract_nginx_stream_value::<NginxBool>(NonNull::new(cf).expect("cf not null"), cmd)
    {
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
    let conf = unsafe { &mut *(conf as *mut ModuleConfig) };
    if let Some(v) = extract_nginx_stream_value::<u64>(NonNull::new(cf).expect("cf not null"), cmd)
    {
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
    let conf = unsafe { &mut *(conf as *mut ModuleConfig) };
    if let Some(v) = extract_nginx_stream_value::<u32>(NonNull::new(cf).expect("cf not null"), cmd)
    {
        conf.cwnd_gain = Some(v);
        NGX_CONF_OK
    } else {
        NGX_CONF_ERROR
    }
}
