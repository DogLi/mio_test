extern crate byteorder;
#[macro_use] extern crate log;
extern crate env_logger;

use std::io::prelude::*;
use std::net::TcpStream;
use std::thread;
use std::io::{self, Read};

use byteorder::{ByteOrder, BigEndian};

static NTHREADS: i32 = 1;

fn wait(){
let mut input = String::new();
    match io::stdin().read_line(&mut input) {
        Ok(n) => {
            println!("-------> {}", input);
        }
        Err(error) => println!("error: {}", error),
    }
}

fn main() {
    for i in 0..NTHREADS {
        let _ = thread::spawn(move || {

            let mut stream = TcpStream::connect("127.0.0.1:8001").unwrap();

            loop {
                let msg = format!("the answer is {}", i);
                let mut buf = [0u8; 8];

                println!("thread {}: Sending over message length of {}", i, msg.len());
                BigEndian::write_u64(&mut buf, msg.len() as u64);
                wait();
                stream.write_all(buf.as_ref()).unwrap();
                wait();
                stream.write_all(msg.as_ref()).unwrap();


                let mut buf = [0u8; 8];
                println!("begin to read stream 3");
                wait();
                stream.read(&mut buf).unwrap();

                let msg_len = BigEndian::read_u64(&mut buf);
                println!("Thread {}: Reading message length of {}", i, msg_len);

                let mut r = [0u8; 256];
                wait();
                let s_ref = <TcpStream as Read>::by_ref(&mut stream);

                match s_ref.take(msg_len).read(&mut r) {
                    Ok(0) => {
                        println!("thread {}: 0 bytes read", i);
                    },
                    Ok(n) => {
                        println!("thread {}: {} bytes read", i, n);

                        let s = std::str::from_utf8(&r[..]).unwrap();
                        println!("thread {} read = {}", i, s);
                    },
                    Err(e) => {
                        panic!("thread {}: {}", i, e);
                    }
                }
               break;// loop onece
            }
        });
    }
    loop {}
}
