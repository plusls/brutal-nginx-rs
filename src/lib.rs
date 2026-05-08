use std::{
    ffi::{c_char, c_void},
    ptr::{self, NonNull},
    str::FromStr,
};

use nginx_sys::{
    __socket_type_SOCK_STREAM, AF_UNIX, IPPROTO_TCP, NGX_CONF_TAKE1, NGX_LOG_EMERG, NGX_LOG_ERR,
    NGX_LOG_INFO, NGX_STREAM_SRV_CONF, TCP_CONGESTION, ngx_command_t, ngx_conf_t, ngx_int_t,
    ngx_module_t, ngx_str_t, ngx_uint_t, setsockopt,
};
use ngx::{
    core::{NGX_CONF_ERROR, NGX_CONF_OK, Status},
    http::{Merge, MergeConfigError},
    ngx_conf_log_error, ngx_log_error, ngx_string,
};

use crate::value::{
    ComplexValueTrait, NginxBool, NginxComplexValue, NginxHandlerCtxTrait, NginxValue,
};
#[cfg(feature = "export-modules")]
use crate::{brutal_http::ngx_http_brutal_module, brutal_stream::ngx_stream_brutal_module};

pub mod value;

#[cfg(ngx_feature = "stream")]
pub mod stream;

#[derive(Debug)]
pub struct ModuleConfig<CVT, T> {
    pub enable: Option<NginxValue<NginxBool, CVT>>,
    pub rate: Option<NginxValue<u64, CVT>>,
    pub cwnd_gain: Option<NginxValue<u32, CVT>>,
    pub extra: Option<T>,
}

impl<CVT, T> Default for ModuleConfig<CVT, T> {
    fn default() -> Self {
        Self {
            enable: Default::default(),
            rate: Default::default(),
            cwnd_gain: Default::default(),
            extra: Default::default(),
        }
    }
}

impl<CVT, T> ModuleConfig<CVT, T> {
    pub fn get_brutal_param<CTX: NginxHandlerCtxTrait<NginxComplexValueType = CVT>>(
        &mut self,
        ctx: &mut CTX,
    ) -> Option<BrutalParams> {
        if let Some(enable) = &mut self.enable {
            match enable.extract(Some(ctx)) {
                Ok(enable) => {
                    if !enable.as_bool() {
                        return None;
                    }
                }
                Err(err) => {
                    ngx_log_error!(NGX_LOG_EMERG, ctx.log(), "{err}",);
                    return None;
                }
            }
        } else {
            return None;
        }

        if let Some(rate) = &mut self.rate {
            let mut cwnd_gain_default = NginxValue::Static(15);
            let cwnd_gain = self.cwnd_gain.as_mut().unwrap_or(&mut cwnd_gain_default);

            let rate = match rate.extract(Some(ctx)) {
                Ok(rate) => rate,
                Err(err) => {
                    ngx_log_error!(NGX_LOG_EMERG, ctx.log(), "{err}",);
                    return None;
                }
            };

            let cwnd_gain = match cwnd_gain.extract(Some(ctx)) {
                Ok(cwnd_gain) => cwnd_gain,
                Err(err) => {
                    ngx_log_error!(NGX_LOG_EMERG, ctx.log(), "{err}",);
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
pub struct BrutalParams {
    rate: u64,
    cwnd_gain: u32,
}

impl<CVT: Clone, T> Merge for ModuleConfig<CVT, T> {
    fn merge(&mut self, prev: &ModuleConfig<CVT, T>) -> Result<(), MergeConfigError> {
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

pub fn brutal_handler<T: NginxHandlerCtxTrait>(
    ctx: &mut T,
    brutal_param: Option<BrutalParams>,
) -> Status {
    let connection = ctx.connection();
    let should_apply_brutal = connection.type_ as u32 == __socket_type_SOCK_STREAM
        && unsafe {
            connection
                .listening
                .as_ref()
                .and_then(|listening| listening.sockaddr.as_ref())
                .is_some_and(|sockaddr| sockaddr.sa_family as u32 != AF_UNIX)
        };
    let log = ctx.log();
    if let Some(brutal_param) = brutal_param
        && should_apply_brutal
    {
        ngx_log_error!(NGX_LOG_INFO, log, "brutal module enabled.");
        let fd = connection.fd;
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
    }
    Status::NGX_DECLINED
}

pub fn extract_nginx_value<T: FromStr, CVT: ComplexValueTrait>(
    mut cf: NonNull<ngx_conf_t>,
    cmd: NonNull<ngx_command_t>,
) -> Option<NginxValue<T, CVT>> {
    let args: &mut [ngx_str_t] = unsafe { (*cf.as_ref().args).as_slice_mut() };
    let cf = unsafe { cf.as_mut() };
    let cmd_name = unsafe { cmd.as_ref() }.name;
    let cmd_name = if let Ok(cmd_name) = cmd_name.to_str() {
        cmd_name
    } else {
        ngx_conf_log_error!(NGX_LOG_EMERG, cf, "cmd name {} not utf-8 encoded", cmd_name);
        return None;
    };

    let mut cv: CVT = unsafe { std::mem::zeroed::<CVT>() };
    let mut ccv: CVT::NginxCompileComplexValueType = unsafe { std::mem::zeroed() };
    CVT::init_compile_complex_value(&mut ccv, cf, &args[1], &mut cv);

    if let Err(err) = CVT::compile(&mut ccv) {
        ngx_conf_log_error!(
            NGX_LOG_EMERG,
            cf,
            "`{cmd_name}` argument compile failed. err: {err}"
        );
        return None;
    }

    match cv.static_value::<T>() {
        Ok(Some(static_value)) => {
            // 静态值
            Some(NginxValue::Static(static_value))
        }
        Err(err) => {
            // 静态值解析失败
            ngx_conf_log_error!(NGX_LOG_EMERG, cf, "invalid {cmd_name}, err: {err}",);
            None
        }
        Ok(None) => {
            let conf_file = cf.conf_file;
            let (file_name, line) = if let Some(conf_file) = NonNull::new(conf_file) {
                let conf_file = unsafe { conf_file.as_ref() };
                let file_name = if let Ok(file_name) = conf_file.file.name.to_str() {
                    file_name.to_string()
                } else {
                    ngx_conf_log_error!(
                        NGX_LOG_EMERG,
                        cf,
                        "invalid config file name {}",
                        conf_file.file.name
                    );
                    return None;
                };
                (file_name, conf_file.line as u32)
            } else {
                ngx_conf_log_error!(NGX_LOG_EMERG, cf, "conf_file name is null",);
                return None;
            };
            // complex value，里面有变量
            let val = NginxValue::Complex(NginxComplexValue {
                cv,
                cmd_name: cmd_name.to_string(),
                file_name,
                line,
            });
            Some(val)
        }
    }
}

struct Module;

mod brutal_stream {
    use std::ptr::null_mut;

    use super::*;
    use crate::{
        Module,
        stream::{
            self, NgxStreamCoreModule, Session, StreamModule as _, StreamModuleServerConf,
            StreamPhase, StreamSessionHandler,
        },
    };
    use nginx_sys::{
        NGX_STREAM_MODULE, NGX_STREAM_SRV_CONF_OFFSET, ngx_stream_complex_value_t,
        ngx_stream_module_t, ngx_stream_session_t,
    };

    // ssl_preload 的解析位于 Preread 的最后, 因此需要在 Preread 后执行
    // 但是 Preread 后就是 Content 阶段, 只能替换该阶段的 handler, 做一个 wrapper
    // 坑点:
    // 1. preload 的变量是 cached 的, 若是在变量初始化完成前去读取, 会导致其永远未初始化
    // 2. preload 模块执行结束后会强制跳到下一阶段, 因此没法在 preread 阶段处理 handler
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
    type ContentHandler = unsafe extern "C" fn(*mut nginx_sys::ngx_stream_session_s);
    unsafe impl StreamModuleServerConf for Module {
        type ServerConf = ModuleConfig<ngx_stream_complex_value_t, ContentHandler>;
    }

    pub struct BrutalSessionHandler;
    impl StreamSessionHandler for BrutalSessionHandler {
        const PHASE: StreamPhase = StreamPhase::Preread;
        type Output = Status;

        // 判断是否存在 content handler, 如果有的话保存旧的 handler, 替换为自己的
        fn handler(session: &mut Session) -> Self::Output {
            // 不能在这解析 brutual_params, 在这里解析不到 ssl preload 的变量
            let conf = Module::server_conf_mut(session).expect("module config is none");
            session.set_module_ctx(null_mut(), Module::module());

            let cscf = NgxStreamCoreModule::server_conf_mut(session).expect("stream core srv conf");
            if let (Some(enable), Some(_)) = (&conf.enable, &conf.rate) {
                match enable {
                    // 对于需要解析的情况留到 content 阶段解析
                    NginxValue::Complex(_) | NginxValue::Static(NginxBool(true)) => {}
                    NginxValue::Static(NginxBool(false)) => {
                        return Status::NGX_DECLINED;
                    }
                }
            }
            if let Some(old_content_handler_ref) = &mut cscf.handler
                && !std::ptr::fn_addr_eq(
                    *old_content_handler_ref,
                    raw_stream_content_brutal_handler
                        as unsafe extern "C" fn(*mut ngx_stream_session_t),
                )
            {
                let old_content_handler = *old_content_handler_ref;
                // let addr = unsafe { session.as_ref().ctx.add(Module::module().ctx_index) } as u64;
                // eprintln!(
                //     "wtf store ptr to {addr:#x} old_handler: {:#x}, raw handler: {:#x}, session: {:#x}, ctx: {:#x}",
                //     old_content_handler as usize,
                //     raw_stream_content_brutal_handler as *const () as usize,
                //     session.as_ref() as *const _ as usize,
                //     session.as_ref().ctx as usize,
                // );
                // 存储旧的函数指针
                // 存储到配置文件中是因为原本该指针就位于配置文件中
                // 配置文件的生命周期和 session 不同, 多个 session 可能对应一个配置文件, 因此会有并发访问的问题
                // 但是 stream 模块的语义可以保证, 一个配置块最多只有一个 content_handler, 因此这么实现大概不会有问题?
                // 在这替换还有个原因是, 配置解析时无法保证解析顺序, 因此 content_handler 不一定已经设置好了
                // 必须先存进 conf 再设置 old_content_handler_ref, 因为 old_content_handler_ref 可能会被并发访问
                conf.extra = Some(old_content_handler);
                *old_content_handler_ref = raw_stream_content_brutal_handler;
            }
            Status::NGX_DECLINED
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

    #[used]
    #[allow(non_upper_case_globals)]
    #[cfg_attr(not(feature = "export-modules"), unsafe(no_mangle))]
    pub static mut ngx_stream_brutal_module: ngx_module_t = ngx_module_t {
        ctx: &raw const NGX_STREAM_BRUTAL_MODULE_CTX as _,
        commands: unsafe { &raw mut NGX_STREAM_BRUTAL_COMMANDS[0] },
        type_: NGX_STREAM_MODULE as _,
        ..ngx_module_t::default()
    };

    unsafe extern "C" fn raw_stream_content_brutal_handler(s: *mut ngx_stream_session_t) {
        let session = unsafe { Session::from_ngx_stream_session(s) };
        let conf = Module::server_conf_mut(session).expect("module config is none");
        let brutual_params = conf.get_brutal_param(session);
        brutal_handler(session, brutual_params);

        // let addr = unsafe { session.as_ref().ctx.add(Module::module().ctx_index) } as u64;
        // eprintln!(
        //     "try read ptr from {addr:#x}, session: {:#x}, ctx: {:#x}, bt: {:?},",
        //     s as usize,
        //     session.as_ref().ctx as usize,
        //     std::backtrace::Backtrace::force_capture()
        // );

        // 因为偷懒直接存的函数指针, 现有 api 无法直接拿出来
        // 因此直接 transmute
        let old_handler = conf.extra.expect("old handler always not null");

        // eprintln!("old_handler in raw: {:#x}", old_handler as usize);
        unsafe { old_handler(s) }
    }

    extern "C" fn ngx_stream_brutal_set(
        cf: *mut ngx_conf_t,
        cmd: *mut ngx_command_t,
        conf: *mut c_void,
    ) -> *mut c_char {
        let conf = unsafe {
            &mut *(conf as *mut ModuleConfig<ngx_stream_complex_value_t, ContentHandler>)
        };
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
        let conf = unsafe {
            &mut *(conf as *mut ModuleConfig<ngx_stream_complex_value_t, ContentHandler>)
        };
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
        let conf = unsafe {
            &mut *(conf as *mut ModuleConfig<ngx_stream_complex_value_t, ContentHandler>)
        };
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
}

mod brutal_http {
    use super::*;
    use crate::Module;
    use nginx_sys::{
        NGX_HTTP_LOC_CONF, NGX_HTTP_LOC_CONF_OFFSET, NGX_HTTP_MODULE, ngx_http_complex_value_t,
        ngx_http_module_t,
    };
    use ngx::http::{
        self, HttpModule as _, HttpModuleLocationConf, HttpPhase, HttpRequestHandler, Request,
    };
    impl http::HttpModule for Module {
        fn module() -> &'static ngx_module_t {
            unsafe { &*::core::ptr::addr_of!(ngx_http_brutal_module) }
        }

        unsafe extern "C" fn postconfiguration(cf: *mut ngx_conf_t) -> ngx_int_t {
            // SAFETY: this function is called with non-NULL cf always
            let cf = unsafe { &mut *cf };
            http::add_phase_handler::<BrutalHandler>(cf)
                .map_or(Status::NGX_ERROR, |_| Status::NGX_OK)
                .into()
        }
    }

    unsafe impl HttpModuleLocationConf for Module {
        type LocationConf = ModuleConfig<ngx_http_complex_value_t, ()>;
    }

    pub struct BrutalHandler;
    impl HttpRequestHandler for BrutalHandler {
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
            type_: (NGX_HTTP_LOC_CONF | NGX_CONF_TAKE1) as ngx_uint_t,
            set: Some(ngx_http_brutal_set),
            conf: NGX_HTTP_LOC_CONF_OFFSET,
            offset: 0,
            post: ptr::null_mut(),
        },
        ngx_command_t {
            name: ngx_string!("brutal_rate"),
            type_: (NGX_HTTP_LOC_CONF | NGX_CONF_TAKE1) as ngx_uint_t,
            set: Some(ngx_http_brutal_rate_commands_set),
            conf: NGX_HTTP_LOC_CONF_OFFSET,
            offset: 0,
            post: ptr::null_mut(),
        },
        ngx_command_t {
            name: ngx_string!("brutal_cwnd_gain"),
            type_: (NGX_HTTP_LOC_CONF | NGX_CONF_TAKE1) as ngx_uint_t,
            set: Some(ngx_http_brutal_cwnd_gain_commands_set),
            conf: NGX_HTTP_LOC_CONF_OFFSET,
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
        let conf = unsafe { &mut *(conf as *mut ModuleConfig<ngx_http_complex_value_t, ()>) };
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
        let conf = unsafe { &mut *(conf as *mut ModuleConfig<ngx_http_complex_value_t, ()>) };
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
        let conf = unsafe { &mut *(conf as *mut ModuleConfig<ngx_http_complex_value_t, ()>) };
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
}

macro_rules! ngx_modules {
    ($( $mod:ident ),+) => {
        #[unsafe(no_mangle)]
        #[allow(non_upper_case_globals)]
        pub static mut ngx_modules: [*const ngx::ffi::ngx_module_t; count!($( $mod, )+) + 1] = [
            $( &raw const $mod as *const ngx::ffi::ngx_module_t, )+
            ::core::ptr::null()
        ];

        #[unsafe(no_mangle)]
        #[allow(non_upper_case_globals)]
        pub static mut ngx_module_names: [*const ::core::ffi::c_char; count!($( $mod, )+) + 1] = [
            $( concat!(stringify!($mod), "\0").as_ptr() as *const ::core::ffi::c_char, )+
            ::core::ptr::null()
        ];

        #[unsafe(no_mangle)]
        #[allow(non_upper_case_globals)]
        pub static mut ngx_module_order: [*const ::core::ffi::c_char; 1] = [
            ::core::ptr::null()
        ];
    };
}

macro_rules! replace_expr {
    ($_t:tt $sub:expr) => {
        $sub
    };
}

macro_rules! count {
    ($($x:ident),+ $(,)?) => {
        0usize $(+ replace_expr!($x 1usize))+
    };
}

// nginx 仓库的宏有bug, 因此自己改了一下
// Generate the `ngx_modules` table with exported modules.
// This feature is required to build a 'cdylib' dynamic module outside of the NGINX buildsystem.
#[cfg(feature = "export-modules")]
ngx_modules!(ngx_stream_brutal_module, ngx_http_brutal_module);
