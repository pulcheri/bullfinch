extern crate crossbeam_channel;

use reqwest::header::*;
use reqwest::{Client, Url};
use std::collections::{HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc};
use std::thread;
use std::time::Duration;
//use std::sync::mpsc::channel;
use crossbeam_channel as channel;

pub mod error;
pub use error::BfError;

mod urlscraper;
use urlscraper::UrlScraper;


fn is_same_domain(domain: &str, url: &Url) -> bool {
    match url.domain() {
        Some(domain_) => domain_ == domain,
        None => false,
    }
}


/// Single domain crawler
#[derive(Debug)]
pub struct Crawler {
    domain: String,
    domain_url: Url,
    pub visited: HashSet<Url>,
    //pub frontier: VecDeque<Url>,
    /// Number of fetching threads
    num_fetchers: usize,
    pub crawl_depth: usize,
    /// Log links as they are processed or not (currently uses println)
    pub verbose_log: bool,
}

/*
TODO serialize this and use in Crawler
#[derive(Debug)]
pub struct Store {
    pub visited: HashSet<Url>,
}
*/

impl Crawler {
    /// Create new crawler with empty data
    pub fn new(domain: &str) -> Result<Self, BfError> {
        // Parse string and extract domain part
        let url = match Url::parse(domain) {
            Ok(url_) => Ok(url_),
            Err(err) => Err(BfError::UrlError(err))
        }?;
        let domain_str = url.domain().unwrap();
        let d: String = String::from(domain_str);
        //let v = Store { visited: HashSet::new() };
        let v = HashSet::new();
        Ok(Self {
            domain: d,
            domain_url: url,
            visited: v,
            //frontier: f,
            num_fetchers: 2,
            crawl_depth: 2,
            verbose_log: true,
        })
    }

    pub fn start(&mut self) {
        // If we encounter a link then send it to a master thread
        // it will run in a single thread whilst multiple worker threads will be fetching them
        let (master_tx, master_rx) = channel::unbounded::<(usize, Url)>();

        // After it has been verified that this is a valid link and we have not seen it yet
        // then send to worker threads for fetching
        let (worker_tx, worker_rx) = channel::unbounded::<(usize, Url)>();

        let client_ = Arc::new(Client::new());

        // Start crawling from the root
        match master_tx.send((0, self.domain_url.clone())) {
            Err(err) => println!("Error sending to master: {}", err),
            _ => (),
        }

        // Flag to signal worker threads to shut down
        let shutdown = Arc::new(AtomicBool::new(false));

        for _ in 0..self.num_fetchers {
            let worker_rx = worker_rx.clone();
            let master_tx = master_tx.clone();
            let client = Arc::clone(&client_);
            let domain = self.domain.clone();
            let shutdown = Arc::clone(&shutdown);
            let max_depth = self.crawl_depth;

            thread::spawn(move || {
                for (depth, url) in worker_rx {
                    if shutdown.load(Ordering::SeqCst) {
                        println!("Worker thread shutting down.");
                        break;
                    }
                    if depth >= max_depth {
                        continue;
                    }

                    let head = match client.head(url.clone()).send() {
                        Ok(head) => head,
                        Err(err) => {
                            println!("Error in getting head of {} : {}", url.as_str(), err);
                            continue;
                        }
                    };
                    let headers = head.headers();
                    if let Some(content_type) =
                        headers.get(CONTENT_TYPE).and_then(|c| c.to_str().ok())
                    {
                        if content_type.starts_with("text/html") {
                            // If this is a html page then get it ...
                            let mut resp = client.get(url.clone()).send().unwrap();
                            let text = resp.text().unwrap();
                            // .. and parse
                            let url_scraper = UrlScraper::new(url, &text).unwrap();
                            // send all the links to master for processing
                            for link in url_scraper
                                .into_iter()
                                .filter(|u| is_same_domain(&domain, u))
                            {
                                match master_tx.send((depth + 1, link)) {
                                    Err(err) => {
                                        println!("Error sending to master: {}", err);
                                    }
                                    _ => (),
                                }
                            }
                        }
                    }
                }
            });
        }


        while !shutdown.load(Ordering::SeqCst) {
            let (depth, url) = match master_rx.try_recv() {
                Ok((depth, url)) => (depth, url),
                Err(_err) => {
                    // TODO
                    // since worker threads might be working give them a chance to complete
                    thread::sleep(Duration::from_millis(2_000));
                    //println!("Error receiving {}", err);
                    // If all channels are empty then there is no work to be done
                    if worker_tx.is_empty() && worker_rx.is_empty() && master_rx.is_empty() {
                        shutdown.store(true, Ordering::SeqCst);
                        // FIXME worker threads might still be stuck in loading data
                        println!("No more work to do. Shutting down.");
                    }
                    continue;
                }
            };

            if !is_same_domain(&self.domain_url.domain().unwrap(), &url) {
                println!(
                    "Different domain: {} {}",
                    url,
                    &self.domain_url.domain().unwrap()
                );
                continue;
            }

            // Remove fragment
            let mut url_f = url.clone();
            url_f.set_fragment(None);
            if self.visited.contains(&url_f) {
                if self.verbose_log {
                    //println!("Already visited {}", url);
                }
            } else {
                if self.verbose_log {
                    println!("Visiting: {} {}", depth, url);
                }
                self.visited.insert(url_f);
                
                if depth >= self.crawl_depth {
                    continue
                }

                match worker_tx.send((depth, url)) {
                    Err(err) => {
                        println!("Error sending to worker channel {}. Quitting.", err);
                        shutdown.store(true, Ordering::SeqCst);
                    }
                    _ => ()
                };
                // Intentionally slowing down
                // Let's behave responsibly
                thread::sleep(Duration::from_millis(250));
            }
        }

    }
}