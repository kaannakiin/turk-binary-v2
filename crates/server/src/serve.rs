use std::future::IntoFuture;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use route::PoolFeed;
use tokio::net::TcpListener;

use crate::api::{self, Api};
use crate::error::ServerError;
use crate::executor::SearchPool;
use crate::health::Health;
use crate::ops;
use crate::service::QuoteSlot;
use crate::settings::ServerSettings;

/// `/health` and `/ready`, on their own address so probes and metrics stay
/// off the API port.
pub struct OpsServer {
    listener: TcpListener,
    router: Router,
    health: Health,
    shutdown_timeout: Duration,
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
            shutdown_timeout: settings.shutdown_timeout(),
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, ServerError> {
        self.listener.local_addr().map_err(ServerError::LocalAddr)
    }

    pub async fn run(self) -> Result<(), ServerError> {
        tokio::select! {
            result = serve(self.listener, self.router, &self.health, self.shutdown_timeout) => result,
            () = self.health.track_clock() => Ok(()),
        }
    }
}

/// `POST /route`. Bound and answering from the start: requests before the
/// quote reader is attached get `NOT_READY` rather than a hung connection.
pub struct ApiServer {
    listener: TcpListener,
    router: Router,
    health: Health,
    shutdown_timeout: Duration,
}

impl ApiServer {
    pub async fn bind<F: PoolFeed>(
        settings: &ServerSettings,
        health: Health,
        pool: SearchPool,
        quotes: QuoteSlot<F>,
    ) -> Result<Self, ServerError> {
        settings.quote.validate()?;
        let api = Api {
            pool: Arc::new(pool),
            quotes,
            settings: settings.quote,
        };
        Ok(Self {
            listener: bind(settings.api_addr).await?,
            router: api::router(api),
            health,
            shutdown_timeout: settings.shutdown_timeout(),
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, ServerError> {
        self.listener.local_addr().map_err(ServerError::LocalAddr)
    }

    pub async fn run(self) -> Result<(), ServerError> {
        serve(
            self.listener,
            self.router,
            &self.health,
            self.shutdown_timeout,
        )
        .await
    }
}

async fn bind(addr: SocketAddr) -> Result<TcpListener, ServerError> {
    TcpListener::bind(addr)
        .await
        .map_err(|source| ServerError::Bind { addr, source })
}

/// Serves until [`Health::stop`], then waits up to `timeout` for requests
/// in flight.
async fn serve(
    listener: TcpListener,
    router: Router,
    health: &Health,
    timeout: Duration,
) -> Result<(), ServerError> {
    let stopping = health.clone();
    let serve = axum::serve(listener, router)
        .with_graceful_shutdown(async move { stopping.stopping().await })
        .into_future();
    let deadline = async {
        health.stopping().await;
        tokio::time::sleep(timeout).await;
    };
    tokio::select! {
        result = serve => result.map_err(ServerError::Serve),
        () = deadline => {
            tracing::warn!(?timeout, "requests still open at shutdown");
            Ok(())
        }
    }
}
