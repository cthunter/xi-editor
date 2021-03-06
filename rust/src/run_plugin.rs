// Copyright 2016 Google Inc. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Module to run a plugin.

use std::io::BufReader;
use std::env;
use std::path::PathBuf;
use std::process::{Command,Stdio,ChildStdin};
use std::sync::{Arc, Mutex, Weak};
use std::thread;
use serde_json::Value;
use serde_json::builder::ObjectBuilder;

use xi_rpc::{RpcLoop, RpcPeer, dict_get_u64};
use editor::Editor;

pub type PluginPeer = RpcPeer<ChildStdin>;

pub struct PluginRef(Arc<Mutex<Plugin>>);

pub struct Plugin {
    editor: Weak<Mutex<Editor>>,
    peer: PluginPeer,
}

pub fn start_plugin(editor: Arc<Mutex<Editor>>) {
    thread::spawn(move || {
        let mut pathbuf: PathBuf = match env::current_exe() {
            Ok(pathbuf) => pathbuf,
            Err(e) => {
                print_err!("Could not get current path: {}", e);
                return;
            }
        };
        pathbuf.pop();
        pathbuf.push("xi-syntect-plugin");
        //print_err!("path = {:?}", pathbuf);
        let mut child = Command::new(&pathbuf)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("plugin failed to start");
        let child_stdin = child.stdin.take().unwrap();
        let child_stdout = child.stdout.take().unwrap();
        let mut looper = RpcLoop::new(child_stdin);
        let peer = looper.get_peer();
        peer.send_rpc_notification("ping", &Value::Array(Vec::new()));
        let plugin = Plugin {
            editor: Arc::downgrade(&editor),
            peer: peer,
        };
        let plugin_ref = PluginRef(Arc::new(Mutex::new(plugin)));
        editor.lock().unwrap().on_plugin_connect(plugin_ref.clone());
        looper.mainloop(|| BufReader::new(child_stdout),
            |method, params| plugin_ref.rpc_handler(method, params));
        let status = child.wait();
        print_err!("child exit = {:?}", status);
    });
}

impl PluginRef {
    fn rpc_handler(&self, method: &str, params: &Value) -> Option<Value> {
        let editor = {
            self.0.lock().unwrap().editor.upgrade()
        };
        if let Some(editor) = editor {
            let mut editor = editor.lock().unwrap();
            match method {
                // TODO: parse json into enum first, just like front-end RPC
                // (this will also improve error handling, no panic on malformed request from plugin)
                "n_lines" => Some(Value::U64(editor.plugin_n_lines() as u64)),
                "get_line" => {
                    let line = params.as_object().and_then(|dict| dict.get("line").and_then(Value::as_u64)).unwrap();
                    let result = editor.plugin_get_line(line as usize);
                    Some(Value::String(result))
                }
                "get_data" => {
                    params.as_object().and_then(|dict|
                        dict_get_u64(dict, "offset").and_then(|offset|
                            dict_get_u64(dict, "max_size").and_then(|max_size|
                                dict_get_u64(dict, "rev").and_then(|rev| {
                                    let result = editor.plugin_get_data(offset as usize,
                                            max_size as usize, rev as usize);
                                    result.map(|s| Value::String(s))
                                })
                            )
                        )
                    )
                }
                "set_line_fg_spans" => {
                    let dict = params.as_object().unwrap();
                    let line_num = dict.get("line").and_then(Value::as_u64).unwrap() as usize;
                    let spans = dict.get("spans").unwrap();
                    editor.plugin_set_line_fg_spans(line_num, spans);
                    None
                }
                "alert" => {
                    let msg = params.as_object().and_then(|dict| dict.get("msg").and_then(Value::as_str)).unwrap();
                    editor.plugin_alert(msg);
                    None
                }
                _ => {
                    print_err!("unknown plugin callback method: {}", method);
                    None
                }
            }
        } else {
            // connection to editor lost
            None  // TODO: return error value
        }
    }

    pub fn init_buf(&self, buf_size: usize, rev: usize) {
        let plugin = self.0.lock().unwrap();
        let params = ObjectBuilder::new()
            .insert("buf_size", buf_size as u64)
            .insert("rev", rev as u64)
            .build();
        plugin.peer.send_rpc_notification("init_buf", &params);
    }

    // TODO: send finer grain delta
    // TODO: make this a synchronous request (but with a callback to not block),
    // so editor can defer gc until request returns
    pub fn update(&self, buf_size: usize, rev: usize) {
        let plugin = self.0.lock().unwrap();
        let params = ObjectBuilder::new()
            .insert("buf_size", buf_size as u64)
            .insert("rev", rev as u64)
            .build();
        plugin.peer.send_rpc_notification("update", &params);
    }
}

impl Clone for PluginRef {
    fn clone(&self) -> Self {
        PluginRef(self.0.clone())
    }
}
