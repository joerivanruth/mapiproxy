use std::{
    borrow::BorrowMut,
    cell::RefCell,
    collections::HashMap,
    fmt, io,
    ops::{Deref, DerefMut},
    rc::Rc,
    sync::atomic::AtomicUsize,
    time::Duration,
};

use mio::{event::Source, Events, Interest, Poll, Registry, Token};

pub struct Reactor<A> {
    poll: Rc<RefCell<Poll>>,
    token_counter: RefCell<usize>,
    token_activity: Rc<RefCell<HashMap<Token, A>>>,
    drop_activity: Rc<dyn Fn(Token)>,
}

impl<A: fmt::Debug + Copy + Clone + 'static> Reactor<A> {
    pub fn new() -> io::Result<Self> {
        let poll = Poll::new()?;
        let token_activity = Rc::new(RefCell::new(HashMap::default()));
        let handle: Rc<RefCell<HashMap<Token, A>>> = Rc::clone(&token_activity);
        let drop_activity = move |token: Token| {
            let mut mapping = RefCell::borrow_mut(&*handle);
            mapping.remove(&token);
        };
        let reactor = Reactor {
            poll: Rc::new(RefCell::new(poll)),
            token_counter: RefCell::new(0),
            token_activity,
            drop_activity: Rc::new(drop_activity),
        };
        Ok(reactor)
    }

    pub fn activity(&self, token: Token) -> Option<A> {
        let mapping = RefCell::borrow_mut(&self.token_activity);
        mapping.get(&token).copied()
    }

    pub fn register<S: Source>(&self, activity: A, source: S) -> Registered<S> {
        let token = {
            let mut guard = RefCell::borrow_mut(&self.token_counter);
            let n = *guard;
            *guard += 1;
            Token(n)
        };
        let mut mapping = RefCell::borrow_mut(&self.token_activity);
        if let Some(prev) = mapping.insert(token, activity) {
            panic!("cannot assign token {token:?} to activity {activity:?}, already assigned to {prev:?}");
        }
        Registered {
            poll: Rc::clone(&self.poll),
            token,
            source,
            interest: None,
            drop_activity: Rc::clone(&self.drop_activity),
        }
    }

    pub fn poll(&mut self, events: &mut Events, timeout: Option<Duration>) -> io::Result<()> {
        let mut poll_guard = RefCell::borrow_mut(&self.poll);
        poll_guard.poll(events, timeout)
    }
}

pub struct Registered<S: Source> {
    poll: Rc<RefCell<Poll>>,
    token: Token,
    source: S,
    interest: Option<Interest>,
    drop_activity: Rc<dyn Fn(Token)>,
}

impl<S: Source> Drop for Registered<S> {
    fn drop(&mut self) {
        (self.drop_activity)(self.token);
        if self.interest.is_some() {
            let poll_guard = RefCell::borrow_mut(&self.poll);
            poll_guard
                .registry()
                .deregister(&mut self.source)
                .expect("supposed to be registered");
        }
    }
}

impl<S: Source> Deref for Registered<S> {
    type Target = S;

    fn deref(&self) -> &Self::Target {
        &self.source
    }
}

impl<S: Source> DerefMut for Registered<S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.source
    }
}

impl<S: Source> Registered<S> {
    pub fn token(&self) -> Token {
        self.token
    }

    pub fn register(&mut self, interest: Interest) -> io::Result<()> {
        self.change_registration(Some(interest))
    }

    pub fn deregister(&mut self) -> io::Result<()> {
        self.change_registration(None)
    }

    pub fn change_registration(&mut self, new_interest: Option<Interest>) -> io::Result<()> {
        let poll_guard = RefCell::borrow_mut(&self.poll);
        let registry = poll_guard.registry();
        let token = self.token;
        match (self.interest, new_interest) {
            (None, None) => Ok(()),
            (None, Some(new)) => registry.register(&mut self.source, token, new),
            (Some(_), None) => registry.deregister(&mut self.source),
            (Some(cur), Some(new)) => {
                if cur != new {
                    registry.reregister(&mut self.source, token, new)
                } else {
                    Ok(())
                }
            }
        }
    }
}
