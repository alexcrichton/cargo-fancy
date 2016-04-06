extern crate term;

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::BufReader;
use std::io::prelude::*;
use std::io;
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::str;
use std::sync::{Arc, Mutex, Condvar};
use std::thread;

const WIDTH: usize = 80;
const CLEAR: [u8; WIDTH] = [b' '; WIDTH];

fn main() {
    if let Ok(s) = env::var("__CARGO_FANCY") {
        let me = env::current_exe().unwrap();
        if me.file_name().and_then(|s| s.to_str()) == Some("build-script-build") {
            build_script(&s, &me)
        } else {
            rustc(&s, &me);
        }
    } else {
        build();
    }
}

fn build_script(addr: &str, me: &Path) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.write_all(&[1]).unwrap();

    let mut child = Command::new(me.with_file_name("build-script-build2"))
                            .spawn()
                            .unwrap();
    std::process::exit(child.wait().ok().and_then(|s| s.code()).unwrap_or(1));
}

fn rustc(addr: &str, me: &Path) {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let skip_prefix = args.iter().any(|a| {
        a.starts_with("--print") || a.starts_with("-v")
    });
    let crate_name = args.windows(2)
                         .find(|w| w[0] == "--crate-name")
                         .map(|w| &w[1][..]);
    let build_script = if crate_name == Some("build_script_build") {
        args.windows(2).find(|w| w[0] == "--out-dir").map(|w| {
            Path::new(&w[1]).join("build_script_build")
        })
    } else {
        None
    };

    let mut s = TcpStream::connect(addr).unwrap();
    s.write_all(&[0]).unwrap();
    if let Some(crate_name) = crate_name {
        s.write_all(crate_name.as_bytes()).unwrap();
    } else {
        s.write_all(b"__dummy").unwrap();
    }
    let mut cnt = [0xff; 5];
    assert_eq!(s.read(&mut cnt[1..]).unwrap(), 4);

    let mut child = Command::new("rustc")
                            .arg("--color=always")
                            .arg("-Ztime-passes")
                            .args(&args)
                            .stdout(Stdio::piped())
                            .stderr(Stdio::piped())
                            .spawn()
                            .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    thread::spawn(move || {
        prepend(skip_prefix, cnt, &mut stdout, &mut io::stdout())
    });
    thread::spawn(move || {
        prepend(skip_prefix, cnt, &mut stderr, &mut io::stderr())
    });
    let ret = child.wait().ok().and_then(|s| s.code()).unwrap_or(1);

    if let Some(build_script) = build_script {
        let src = Path::new(&build_script);
        let dst = src.with_file_name("build-script-build2");
        fs::rename(&src, &dst).unwrap();
        fs::hard_link(&me, &src).unwrap();
    }

    std::process::exit(ret);

    fn prepend(skip_prefix: bool,
               cnt: [u8; 5],
               input: &mut Read,
               output: &mut Write) {
        let mut buf = [0; 1024];
        let mut to_write = cnt.to_vec();
        let init = if skip_prefix {0} else {to_write.len()};
        to_write.truncate(init);
        loop {
            let n = input.read(&mut buf).unwrap();
            if n == 0 {
                break
            }
            let mut buf = &buf[..n];
            while let Some(i) = buf.iter().position(|b| *b == b'\n') {
                to_write.extend_from_slice(&buf[..i + 1]);
                output.write_all(&to_write).unwrap();
                to_write.truncate(init);
                buf = &buf[i + 1..];
            }
            to_write.extend_from_slice(buf);
        }
        if to_write.len() > init {
            output.write_all(&to_write).unwrap();
        }
    }
}

struct Context {
    inner: Mutex<Inner>,
    wait: Condvar,
}

struct Inner {
    active: HashMap<u32, String>,
    messages: Vec<(Vec<u8>, bool)>,
    tick: usize,
}

fn build() {
    // skip "cargo-fancy" and "fancy"
    let args = env::args().skip(2).collect::<Vec<_>>();
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();

    let cx = Arc::new(Context {
        inner: Mutex::new(Inner {
            messages: Vec::new(),
            active: HashMap::new(),
            tick: 0,
        }),
        wait: Condvar::new(),
    });

    let cx2 = cx.clone();
    thread::spawn(move || {
        let cx = cx2;
        let mut cnt = 0;
        loop {
            let (mut s, _) = l.accept().unwrap();
            let mut id = [0];
            assert_eq!(s.read(&mut id).unwrap(), 1);
            let name = if id == [0] {
                let mut name = [0; 128];
                let n = s.read(&mut name).unwrap();
                s.write_all(&[
                    (cnt >> 24) as u8,
                    (cnt >> 16) as u8,
                    (cnt >>  8) as u8,
                    (cnt >>  0) as u8,
                ]).unwrap();
                str::from_utf8(&name[..n]).unwrap().to_string()
            } else if id == [1] {
                format!("build script")
            } else {
                panic!("wut");
            };

            if name != "_" && name != "__dummy" {
                cx.start(name, cnt);

                let cx = cx.clone();
                thread::spawn(move || {
                    assert_eq!(s.read(&mut [0; 10]).unwrap(), 0);
                    cx.end(cnt);
                });
            }
            cnt += 1;
            if cnt == '\n' as u32 {
                cnt += 1;
            }
        }
    });

    let mut child = Command::new("cargo")
                            .arg(&args[0])
                            .arg("--color=always")
                            .env("__CARGO_FANCY", addr.to_string())
                            .env("RUSTC", env::current_exe().unwrap())
                            .args(&args[1..])
                            .stdout(Stdio::piped())
                            .stderr(Stdio::piped())
                            .spawn()
                            .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let cx2 = cx.clone();
    thread::spawn(move || process(&mut stdout, &cx2, true));
    let cx2 = cx.clone();
    thread::spawn(move || process(&mut stderr, &cx2, false));

    let cx2 = cx.clone();
    thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::new(0, 300_000_000));
            cx2.tick();
        }
    });


    Term {
        cx: cx,
        on_screen: HashMap::new(),
        lines: Vec::new(),
        stdout: term::stdout().unwrap(),
        stderr: term::stderr().unwrap(),
        width: WIDTH,
        tick: 0,
    }.run();
    std::process::exit(child.wait().ok().and_then(|s| s.code()).unwrap_or(1));

    fn process(input: &mut Read, cx: &Context, stdout: bool) {
        let mut input = BufReader::new(input);
        loop {
            let mut v = Vec::new();
            input.read_until(b'\n', &mut v).unwrap();
            if v.len() == 0 {
                break
            }
            cx.output(v, stdout);
        }
        cx.output(Vec::new(), stdout);
    }
}

impl Context {
    fn start(&self, name: String, cnt: u32) {
        let mut inner = self.inner.lock().unwrap();
        inner.active.insert(cnt, name);
        self.wait.notify_one();
    }

    fn end(&self, cnt: u32) {
        let mut inner = self.inner.lock().unwrap();
        inner.active.remove(&cnt).unwrap();
        self.wait.notify_one();
    }

    fn output(&self, input: Vec<u8>, stdout: bool) {
        let mut inner = self.inner.lock().unwrap();
        inner.messages.push((input, stdout));
        self.wait.notify_one();
    }

    fn tick(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.tick += 1;
        self.wait.notify_one();
    }
}

struct Term {
    cx: Arc<Context>,
    lines: Vec<Line>,
    on_screen: HashMap<u32, usize>,
    stdout: Box<term::Terminal<Output=io::Stdout>>,
    stderr: Box<term::Terminal<Output=io::Stderr>>,
    width: usize,
    tick: usize,
}

struct Line {
    tick: usize,
    running: bool,
    name: String,
    step: usize,
    total: usize,
}

impl Term {
    fn run(&mut self) {
        let cx = self.cx.clone();
        let mut inner = cx.inner.lock().unwrap();
        let mut stdout_done = false;
        let mut stderr_done = false;
        let mut to_remove = Vec::new();
        loop {
            for _ in 0..self.lines.len() {
                self.stdout.cursor_up().unwrap();
            }
            self.stdout.flush().unwrap();

            for (msg, stdout) in inner.messages.drain(..) {
                if msg.len() > 0 {
                    assert_eq!(msg.iter().position(|b| *b == b'\n'),
                               Some(msg.len() - 1));
                }
                let dst = if stdout {
                    &mut self.stdout as &mut io::Write
                } else {
                    &mut self.stderr as &mut io::Write
                };
                if msg.len() == 0 {
                    if stdout {
                        stdout_done = true;
                    } else {
                        stderr_done = true;
                    }
                } else if msg.first() == Some(&0xff) {
                    let a = ((msg[1] as u32) << 24) |
                            ((msg[2] as u32) << 16) |
                            ((msg[3] as u32) <<  8) |
                            ((msg[4] as u32) <<  0);
                    let msg = &msg[5..];
                    let idx = match self.on_screen.get(&a) {
                        Some(i) => *i,
                        None => {
                            if msg.starts_with(b"time:") ||
                               msg.starts_with(b"  time:") {
                                continue
                            }
                            dst.write_all(b"\r").unwrap();
                            dst.write_all(&CLEAR).unwrap();
                            dst.write_all(b"\r").unwrap();
                            dst.write_all(&msg).unwrap();
                            continue
                        }
                    };
                    self.lines[idx].input(msg, dst);
                } else {
                    let mut do_print = true;
                    if String::from_utf8_lossy(&msg).contains("Compiling") {
                        do_print = false;
                    }
                    if do_print {
                        dst.write_all(&CLEAR).unwrap();
                        dst.write_all(b"\r").unwrap();
                        dst.write_all(&msg).unwrap();
                    } else {
                        // out.write_all(&msg).unwrap();
                    }
                }
            }

            for (a, v) in inner.active.iter() {
                if self.on_screen.contains_key(a) {
                    continue
                }
                let line = Line {
                    name: v.to_string(),
                    running: true,
                    step: 0,
                    total: 59,
                    tick: 0,
                };
                let idx = self.lines
                              .iter()
                              .enumerate()
                              .find(|&(_, b)| !b.running)
                              .map(|(a, _)| a)
                              .unwrap_or(self.lines.len());
                if idx < self.lines.len() {
                    self.lines[idx] = line;
                } else {
                    self.lines.push(line);
                }
                self.on_screen.insert(*a, idx);
            }
            for (a, idx) in self.on_screen.iter() {
                if inner.active.contains_key(a) {
                    continue
                }
                to_remove.push((*a, *idx));
            }
            for (a, idx) in to_remove.drain(..) {
                self.on_screen.remove(&a);
                self.lines[idx].running = false;
                self.lines[idx].step = self.lines[idx].total;
            }

            if self.tick != inner.tick {
                for line in self.lines.iter_mut() {
                    line.tick += inner.tick - self.tick;
                }
                self.tick = inner.tick;
            }

            if stdout_done && stderr_done && self.on_screen.len() == 0 {
                break
            }

            for line in self.lines.iter_mut() {
                line.render(self.width, &mut *self.stdout);
                writeln!(self.stdout, "").unwrap();
            }

            self.stdout.flush().unwrap();
            self.stderr.flush().unwrap();

            inner = self.cx.wait.wait(inner).unwrap();
        }

        for _ in 0..self.lines.len() {
            self.stdout.delete_line().unwrap();
        }
    }
}

impl Line {
    fn input(&mut self, msg: &[u8], out: &mut io::Write) {
        if msg.starts_with(b"time:") {
            self.step += 1;
        } else if !msg.starts_with(b"  time:") {
            out.write_all(b"\r").unwrap();
            out.write_all(&CLEAR).unwrap();
            out.write_all(b"\r").unwrap();
            out.write_all(msg).unwrap();
        }
    }

    fn render(&mut self,
              width: usize,
              out: &mut term::Terminal<Output=io::Stdout>) {
        if self.running {
            out.fg(term::color::YELLOW).unwrap();
            let icons = ["⡆", "⠇", "⠋", "⠙", "⠸", "⢰", "⣠", "⣄"];
            let icon = icons[self.tick % icons.len()];
            write!(out, " [{}] ", icon).unwrap();
        } else {
            out.fg(term::color::GREEN).unwrap();
            write!(out, " [✓] ").unwrap();
        }
        out.reset().unwrap();

        let namelen = 15;
        if self.name.len() > namelen {
            write!(out, "{:<1$} [",
                   format!("{}...", &self.name[..namelen - 3]),
                   namelen).unwrap();
        } else {
            write!(out, "{:<1$} [", self.name, namelen).unwrap();
        }

        let remaining = width - (3 + 2 + namelen + 3);

        if self.name.contains("build script") {
            if self.running {
                for i in 0..remaining-2 {
                    if i == self.tick % (remaining - 2) {
                        write!(out, "...").unwrap();
                    } else {
                        write!(out, " ").unwrap();
                    }
                }
            } else {
                for _ in 0..remaining {
                    write!(out, "#").unwrap();
                }
            }
        } else {
            let n = remaining * self.step / self.total;
            for i in 0..remaining {
                if i <= n {
                    write!(out, "#").unwrap();
                } else {
                    write!(out, " ").unwrap();
                }
            }
        }
        write!(out, "]").unwrap();
    }
}




























