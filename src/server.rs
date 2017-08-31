use std::io::{self, ErrorKind};
use std::rc::Rc;
use mio::{Events, Poll, PollOpt, Ready, Token};
use mio::net::TcpListener;
use mio::unix::UnixReady;

use slab;
use connection::Connection;


type Slab<T> = slab::Slab<T, Token>;

pub struct Server {
    // main socket for our server
    sock: TcpListener,

    // token of our server. we keep track of it here instead of dong `const SERVER = Token(0)`
    token: Token,

    // a list of connections _accepted_ by our server
    conns: Slab<Connection>,

    // a list of events to process
    events: Events,
}

impl Server {
    pub fn new(sock: TcpListener) -> Server {
        Server {
            sock: sock,
            // Give our server token a number much larger than our slab capacity.
            // The slab used to track an internal offset, but does not anymore.
            token: Token(10_000_000),

            // SERVER is Token(1), so start after that we can deal with max of 126 connections
            conns: Slab::with_capacity(126),

            // list of events from the poller that the server needs to process
            events: Events::with_capacity(1024),

        }
    }

    pub fn run(&mut self, poll: &mut Poll) -> io::Result<()> {
        // Poll allows a program to monitor a large number of Evented types,
        // waiting until one or more become "ready" for some class of operations;
        // e.g. reading and writing.
        //
        // To use `Poll`, an `Evented` type must first be registered with the `Poll` instance using the
        // register method, supplying readiness interest. The readiness interest tells `Poll` which
        // specific operations on the handle to monitor for readiness. A Token is also passed to the
        // register function. When Poll returns a readiness event, it will include this token. This
        // associates the event with the Evented handle that generated the event.
        self.register(poll)?;

        info!("Server run loop starting ...");
        loop {
            // Wait for readiness events.  The function will block until either
            // at least one readiness event has been received or timeout has elapsed
            let cnt = poll.poll(&mut self.events, None)?;
            let mut i = 0;
            trace!("processing events... cnt = {}, len={}", cnt, self.events.len());

            // Iterate over the notifications. Each event provides the token
            // it was registered (which usually represents, at least, the handle
            // that the event is about) as well as information about what kind
            // of event occurred(readable, writable, signal, etc.)
            while i < cnt {
                // TODO this would be nice if it would turn a Result type. trying
                // to convert into a io::Result runs into a problem because `.ok_or()`
                // expects `std::Result` and not `io::Result`
                let event = self.events.get(i).expect("failed to get event");

                trace!("event = {:?}, index = {:?}", event, i);
                self.ready(poll, event.token(), event.readiness());
                i += 1;
            }

            self.tick(poll);
        }
    }


    /// 将server 的socket注册到poll中
    ///
    pub fn register(&mut self, poll: &mut Poll) -> io::Result<()> {
        poll.register(
            &self.sock,
            self.token,
            Ready::readable(),
            PollOpt::edge()
            ).or_else(|e|{
                error!("Failed to register server {:?}, {:?} ", self.token, e);
                Err(e)
        })
    }

    /// 处理事件
    fn ready(&mut self, poll: &mut Poll, token: Token, event: Ready) {
        debug!("function ready, {:?} event = {:?}", token, event);

        // Unix platforms are able to provide readiness events for additional socket events, such
        // as HUP and error.
        let event = UnixReady::from(event);

        if event.is_error() {
            warn!("Error event for {:?}", token);
            self.find_connection_by_token(token).mark_reset();
            return;
        }

        if event.is_hup() {
            trace!("hup event for {:?}", token);
            self.find_connection_by_token(token).mark_reset();
            return;
        }

        let event = Ready::from(event);

        // 对于Server的写事件不会发生
        // 对于其他的写事件, 调用`conn.writable()`
        if event.is_writable() {
            trace!("Write event for {:?}", token);
            assert!(self.token != token, "Received writable event for Server");

            let conn = self.find_connection_by_token(token);

            if conn.is_reset() {
                info!("{:?} has already ben reset", token)
            }

            conn.writable()
                .unwrap_or_else(|e| {
                    warn!("Write event failed for {:?}, {:?}", token, e);
                    conn.mark_reset();
                });
        }

        // 对于Server的读事件, 表明新的连接请求, 调用`self.accept(poll)`
        // 对于其他的读事件, 调用`self.readable(token)`
        if event.is_readable() {
            trace!("Read event for {:?}", token);
            if self.token == token {
                self.accept(poll);
            } else {
                if self.find_connection_by_token(token).is_reset() {
                    info!("{:?} has already been reset", token);
                    return;
                }

                self.readable(token)
                    .unwrap_or_else(|e| {
                        warn!("Read event failed for {:?}: {:?}", token, e);
                        self.find_connection_by_token(token).mark_reset();
                    });
            }

            if self.token != token {
                self.find_connection_by_token(token).mark_idle();
            }
        }
    }

    /// 接收一个新的connection
    ///
    /// The server will keep track of the new connection and forward any events from the poller
    /// to this connection

    fn accept(&mut self, poll: &mut Poll) {
        debug!("function accept; server accepting new socket");

        loop{
            // Log an error if there is no socket, but otherwise move on so we do not tear down the
            // entire server.
            let sock = match self.sock.accept() {
                Ok((sock, _)) => sock,
                Err(e) => {
                    if e.kind() == ErrorKind::WouldBlock {
                        debug!("function accept; accept encountered WouldBlock");
                    } else {
                        error!("function accept; Failed to accept new socket, {:?}", e);
                    }
                    return;
                }
            };

            // 将connection插入到slab中
            // vacant_entry: 找到一个空的entry用以存储connection, 没找到返回None
            let token = match self.conns.vacant_entry() {
                Some(entry) => {
                    debug!("function accept; registering {:?} with poller", entry.index());
                    let c =  Connection::new(sock, entry.index());
                    // insert returns `Entry<'a, T, I>`
                    entry.insert(c).index()
                },
                None => {
                    error!("Failed to insert connection into slab");
                    return;
                }
            };

            // 注册具体实现在 connection.rs里
            match self.find_connection_by_token(token).register(poll) {
                Ok(_) => {},
                Err(e) => {
                    error!("Failed to register {:?} connection with poller, {:?}", token, e);
                    self.conns.remove(token);
                }
            }
        } // end loop
    } // end function

    /// 将可读事件发送到建立好的connection里
    ///
    /// connection 从poll里用token获得
    /// 一旦读取完成(`connection.readable()`实现), 将信息发送到所有的connections,实现广播
    ///
    fn readable(&mut self, token: Token) -> io::Result<()> {
        debug!("server conn readable; token = {:?}", token);

        while let Some(message) = self.find_connection_by_token(token).readable()?{
            // Rc用来保证线程安全, 让程序在同一时刻,实现同一资源的多个所有权拥有者,
            // 多个拥有者共享资源
            let rc_message = Rc::new(message);
            // Queue up a write for all connected clients.
            for c in self.conns.iter_mut() {
                c.send_message(rc_message.clone())
                    .unwrap_or_else(|e| {
                        error!("Failed to queue message for {:?}: {:?}", c.token, e);
                        c.mark_reset();
                    });
            }
        }
        Ok(())
    }

    /// 从slab中通过token来获取connection
    fn find_connection_by_token(&mut self, token: Token) -> &mut Connection {
        &mut self.conns[token]
    }

    /// 一个周期完结后
    fn tick(&mut self, poll: &mut Poll) {
        trace!("Handling end of tick");

        let mut reset_tokens = Vec::new();
        for c in self.conns.iter_mut() {
            if c.is_reset() {
                reset_tokens.push(c.token);
            } else if c.is_idle() {
                c.reregister(poll)
                    .unwrap_or_else(|e| {
                        warn!("Reregister failed {:?}", e);
                        c.mark_reset();
                        reset_tokens.push(c.token);
                    })
            }
        }

        for token in reset_tokens {
            match self.conns.remove(token) {
                Some(_c) => {
                    debug!("reset connection; token = {:?}", token);
                }
                None => {
                    warn!("Unable to remove connection for {:?}", token);
                }
            }
        }
    }
}
