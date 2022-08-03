use std::collections::HashMap;
use std::sync::RwLock;
use std::sync::Arc;

use wasmer::Exportable;
use wasmer::ImportObject;
use wasmer::{Store, Module, Instance,Value, Resolver};
use wasmer_emscripten::EmEnv;
use wasmer_emscripten::EmscriptenGlobals;
use wasmer_wasi::WasiEnv;

const DEBUG:bool = true;

lazy_static::lazy_static!{
   static ref LIBRARIES:RwLock<HashMap<String, Arc<Instance>>> = RwLock::new(HashMap::new()) ;
}

const HELP_DESCRIPTOR:&str = 
r#"
libwasm
wasm dynamic linker on native platform

USAGE:
    libwasm [options] [executable.wasm] [arguments]

OPTIONS:
    -ld <directory>             Add <directory> to the linker's search path
    -d, --debug                 Debug mode
    -p, --inspect               Print wasm information
    -h, --help                  Print help information

COMMANDS:
    install                     install package
    inspect                     Print wasm module information
    bindgen                     code generation linking wasm library
"#;

static mut SEARCH_PATHS:Vec<&str> = Vec::new();

fn main() {
    let mut wasm_executable = "".to_owned();
    let mut args = Vec::new();

    let mut profiling = false;
    let mut inspect = false;

    let mut env_args = std::env::args();

    // discard first argument
    env_args.next().unwrap();

    let l = env_args.len();
    let mut i = 0;
    while i < l{

        let arg = env_args.next().unwrap();

        if i == 0{
            if arg == "install" {
                todo!("bindgen command")

            } else if arg == "inspect"{
                inspect = true;

            } else if arg == "bindgen"{
                todo!("bindgen command")
            }
        }

        if arg == "-h" || arg == "--help"{
            
            if inspect{
                return;
            }

            println!("{}", HELP_DESCRIPTOR);
            return;

        } else if arg == "-p" || arg == "--profile"{
            profiling = true;

        } else if arg == "-d" || arg == "--debug"{

        } else if arg == "-ld" || arg == "--link-directory"{
            // the next argument is directory
            i+=1;
            let dir = env_args.next().expect("missing <directory> for -ld flag");

            unsafe{SEARCH_PATHS.push(Box::leak(Box::new(dir)))}

        } else{
            wasm_executable = arg;

            args = env_args.map(|a|{Box::leak(Box::new(a)).as_str()}).collect();
            break;
        }

        i+=1;
    }

    if wasm_executable == ""{
        panic!("libwasm: fatal error: no input files.")
    }

    let config = wasmer::Cranelift::new();
    let engine = wasmer::Universal::new(config);
    let engine = engine.features(
        wasmer::Features { 
            threads: true, 
            reference_types: true, 
            simd: true, 
            bulk_memory: true, 
            multi_value: true, 
            tail_call: true, 
            module_linking: false, 
            multi_memory: true, 
            memory64: true, 
            exceptions: true, 
            relaxed_simd: true, 
            extended_const: true 
        }).engine();

    let store = Store::new(&engine);
    let module = Module::from_file(&store, wasm_executable).unwrap();


    // print the profiled information and exit
    if profiling || inspect{
        println!("path: {}", module.name().unwrap_or("unknown"));
        println!("\nimports:");
        for i in module.imports(){
            println!("      {}::{} : {}", i.module(), i.name(), format_ty(&i.ty()));
        }

        println!("\nexports:");

        for i in module.exports(){
            println!("      name: {} ty:{}", i.name(), format_ty(&i.ty()))
        }

        return;
    }

    let mut resolver = CombindedResolver::new();

    // parse arguments

    if wasmer_wasi::is_wasi_module(&module){
        resolver.enable_wasi(module.name().unwrap_or("main"), &module, &args);

        let instance = Instance::new(&module, &resolver).unwrap();

        if DEBUG{
            println!("all dependency resolved, run _start function.");
        }

        let start = instance.exports.get_function("_start").unwrap();
        start.call(&[]).unwrap();

    } else if wasmer_emscripten::is_emscripten_module(&module){
        let (mut env, mut globals) = resolver.enable_emscripten(&module);

        let mut instance = Instance::new(&module, &resolver).unwrap();

        wasmer_emscripten::run_emscripten_instance(&mut instance, &mut env, &mut globals, "./", args, None).unwrap();

    } else{

        let instance = Instance::new(&module, &resolver).unwrap();

        let main = instance.exports.get_function("main").expect("cannot find function main");

        let argc = args.len() as i32;
        let argv = args.as_ptr() as i64;

        main.call(&[Value::I32(argc), Value::I64(argv)]).unwrap();
    }
}

fn resolve_import(name:&str) -> (String, Arc<Instance>){

    if DEBUG{
        println!("resolving module: {}", name);
    }

    let lib = LIBRARIES.read().unwrap();

    if let Some(a) = lib.get(name){

        if DEBUG{
            println!("symbol {} already resolved, load from instance.", name);
        }
        return (name.to_string(), a.clone())

    } else{
        for i in std::fs::read_dir(std::env::current_dir().unwrap()).unwrap(){
            if let Ok(v) = i{

                let path = v.path().canonicalize().unwrap();

                if path.is_file() && path.file_name().unwrap() == name{

                    if DEBUG{
                        println!("library {} found at {}", name, path.as_path().to_str().unwrap())
                    }

                    let store = Store::default();
                    let module = Module::from_file(&store, path).unwrap();

                    let mut resolver = CombindedResolver::new();

                    let instance = 

                    if wasmer_wasi::is_wasi_module(&module){
                        resolver.enable_wasi(module.name().unwrap_or("main"), &module, &[]);
                
                        let instance = Instance::new(&module, &resolver).unwrap();
                
                        if DEBUG{
                            println!("module {} is loaded and ready.", name);
                        }
                        Arc::new(instance)
                
                    } else if wasmer_emscripten::is_emscripten_module(&module){
                        let (mut env, globals) = resolver.enable_emscripten(&module);
                
                        let mut instance = Instance::new(&module, &resolver).unwrap();
                
                        env.set_memory(globals.memory.clone());
                        wasmer_emscripten::set_up_emscripten(&mut instance).unwrap();
                
                        Arc::new(instance)
                    } else{
                
                        let instance = Instance::new(&module, &resolver).unwrap();

                        Arc::new(instance)
                    };

                    drop(lib);
                    LIBRARIES.write().unwrap().insert(name.to_string(), instance.clone());

                    return (name.to_string(), instance)
                }
            }
        }

        panic!("unable to resolve symbol '{}'", name);
    }
}

pub struct CombindedResolver{
    modules:Vec<(String, Arc<Instance>)>,
    env:Option<ImportObject>,
    is_wasi:bool,
    is_emscripten:bool
}

impl CombindedResolver{
    fn new() -> Self{
        return Self { 
            modules: Vec::new(), 
            env: None, 
            is_wasi: false, 
            is_emscripten: false 
        }
    }

    fn enable_wasi(&mut self, name:&str, module:&Module, args:&[&str]) -> WasiEnv{

        if DEBUG{
            println!("wasi environment enabled for {}", name);
        }

        let mut env = wasmer_wasi::WasiState::new(name)
        .args(args)
        .finalize().unwrap();

        self.env = Some(env.import_object(module).unwrap());
        self.is_wasi = true;
        env
    }

    fn enable_emscripten(&mut self, module:&Module) -> (EmEnv, EmscriptenGlobals){
        let mut globals = wasmer_emscripten::EmscriptenGlobals::new(module.store(), module).unwrap();
        let env = wasmer_emscripten::EmEnv::new(&globals.data, HashMap::new());
        let obj = wasmer_emscripten::generate_emscripten_env(module.store(), &mut globals, &env);
        self.env = Some(obj);
        self.is_emscripten = true;
        (env, globals)
    }
}

impl Resolver for CombindedResolver{
    fn resolve(&self, index: u32, module: &str, field: &str) -> Option<wasmer::Export> {
        
        if let Some(env) = &self.env{
            if let Some(v) = env.resolve(index, module, field){

                if DEBUG{
                    println!("{}.{} resolved from environment.", module, field);
                }
                return Some(v)
            };
        }
        
        for (name, instance) in &self.modules{
            if module == name{
                if let Some(ext) = instance.exports.get_extern(field){

                    if DEBUG{
                        println!("{}.{} resolved from module.", module, field);
                    }

                    return Some(ext.to_export())
                }
            }
        };

        if DEBUG{
            println!("{}.{} not loaded, resolving dynamically.", module, field);
        }

        let (name, instance) = resolve_import(module);
        if let Some(ext) = instance.exports.get_extern(field){
            unsafe{((&self.modules) as *const _ as *mut Vec<(String, Arc<Instance>)>).as_mut().unwrap().push((name, instance.clone()))};
            return Some(ext.to_export())
        }
        if DEBUG{
            println!("failed to resolve {} from {}.", field, name);
        }
        None
    }
}


fn format_ty(t:&wasmer::ExternType) -> String{
    match t{
        wasmer::ExternType::Function(f) => {
            format!("fn({}) -> ({})", f.params().iter().map(|t|{t.to_string()}).collect::<Vec<String>>().join(","), f.results().iter().map(|t|{t.to_string()}).collect::<Vec<String>>().join(","))
        },
        wasmer::ExternType::Global(g) => {
            format!("global {} {}", if g.mutability.is_mutable(){
                "var"
            }else{
                "const"
            }, g.ty)
        },
        wasmer::ExternType::Memory(m) => {
            format!("memory {} {}~{}", if m.shared{
                "shared"
            }else{
                ""
            }, m.minimum.0, if let Some(v) = m.maximum{
                v.0.to_string()
            } else{
                "unlimited".to_owned()
            })
        },
        wasmer::ExternType::Table(t) => {
            format!("table {} minimum {}", t.ty, t.minimum)
        }
    }
}