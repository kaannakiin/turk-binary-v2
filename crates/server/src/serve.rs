use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::Request;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::server::graceful::GracefulShutdown;
use hyper_util::service::TowerToHyperService;
use route::PoolFeed;
use tokio::net::TcpListener;
use tokio::task::JoinSet;
use tokio::time::Instant;
use tower::ServiceExt;

use crate::api::{self, Api};
use crate::blockhash::BlockhashSlot;
use crate::error::ServerError;
use crate::executor::SearchPool;
use crate::health::Health;
use crate::ops;
use crate::service::QuoteSlot;
use crate::settings::ServerSettings;

/// An accept that fails (out of file descriptors) is retried after this.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy)]
struct Limits {
    read_timeout: Duration,
    shutdown_timeout: Duration,
}

impl Limits {
    fn new(settings: &ServerSettings) -> Self {
        Self {
            read_timeout: settings.read_timeout(),
            shutdown_timeout: settings.shutdown_timeout(),
        }
    }
}

/// `/health` and `/ready`, on their own address so probes and metrics stay
/// off the API port.
pub struct OpsServer {
    listener: TcpListener,
    router: Router,
    health: Health,
    limits: Limits,
}

impl OpsServer {
    /// Binds before the engine starts, so a taken port fails at once and
    /// probes get answers during the long startup.
    pub async fn bind(settings: &ServerSettings, health: Health) -> Result<Self, ServerError> {
        settings.ready.validate()?;
        Ok(Self {
            listener: bind(settings.ops_addr).await?,
            router: ops::router(health.clone()),
            health,
            limits: Limits::new(settings),
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, ServerError> {
        self.listener.local_addr().map_err(ServerError::LocalAddr)
    }

    pub async fn run(self) -> Result<(), ServerError> {
        serve(self.listener, self.router, &self.health, self.limits).await;
        Ok(())
    }
}

/// `POST /quote`, `/swap-instructions` and `/swap`. Bound and answering from
/// the start: requests before the quote reader is attached get `NOT_READY`
/// rather than a hung connection.
pub struct ApiServer {
    listener: TcpListener,
    router: Router,
    health: Health,
    pool: Arc<SearchPool>,
    limits: Limits,
}

impl ApiServer {
    pub async fn bind<F: PoolFeed>(
        settings: &ServerSettings,
        health: Health,
        pool: SearchPool,
        quotes: QuoteSlot<F>,
        blockhashes: BlockhashSlot,
    ) -> Result<Self, ServerError> {
        settings.quote.validate()?;
        settings.swap.validate()?;
        let pool = Arc::new(pool);
        let api = Api {
            pool: Arc::clone(&pool),
            quotes,
            settings: settings.quote.clone(),
            swap: settings.swap,
            blockhashes,
            max_clock_stall: settings.ready.max_clock_stall(),
            read_timeout: settings.read_timeout(),
        };
        Ok(Self {
            listener: bind(settings.api_addr).await?,
            router: api::router(api),
            health,
            pool,
            limits: Limits::new(settings),
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, ServerError> {
        self.listener.local_addr().map_err(ServerError::LocalAddr)
    }

    /// Returns once every connection has ended and every search has
    /// returned, or at the shutdown deadline, when the connections are
    /// aborted and the searches still running are left to finish unread.
    pub async fn run(self) -> Result<(), ServerError> {
        let deadline = serve(self.listener, self.router, &self.health, self.limits).await;
        if !self.pool.close(deadline).await {
            tracing::warn!("searches still running at shutdown");
        }
        Ok(())
    }
}

async fn bind(addr: SocketAddr) -> Result<TcpListener, ServerError> {
    TcpListener::bind(addr)
        .await
        .map_err(|source| ServerError::Bind { addr, source })
}

/// Serves until [`Health::stop`], then lets requests in flight finish until
/// the shutdown deadline, which it returns. Past it the connections are
/// aborted: dropping a server that spawned them would leave them running.
async fn serve(listener: TcpListener, router: Router, health: &Health, limits: Limits) -> Instant {
    let mut http = http1::Builder::new();
    http.timer(TokioTimer::new())
        .header_read_timeout(limits.read_timeout);
    let graceful = GracefulShutdown::new();
    let mut connections = JoinSet::new();
    let stopping = health.stopping();
    tokio::pin!(stopping);
    loop {
        tokio::select! {
            () = &mut stopping => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let service = TowerToHyperService::new(
                        router
                            .clone()
                            .map_request(|request: Request<Incoming>| request.map(Body::new)),
                    );
                    let connection =
                        graceful.watch(http.serve_connection(TokioIo::new(stream), service));
                    connections.spawn(async move {
                        if let Err(error) = connection.await {
                            tracing::debug!(%error, "connection ended with an error");
                        }
                    });
                }
                Err(error) => {
                    tracing::warn!(%error, "accepting a connection failed");
                    tokio::time::sleep(ACCEPT_BACKOFF).await;
                }
            },
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
    drop(listener);
    let deadline = Instant::now() + limits.shutdown_timeout;
    if tokio::time::timeout_at(deadline, graceful.shutdown())
        .await
        .is_err()
    {
        tracing::warn!(
            timeout = ?limits.shutdown_timeout,
            "requests still open at shutdown; aborting them"
        );
    }
    connections.shutdown().await;
    deadline
}
