mod control;
mod intercom;
mod settings;
mod state;
mod stats;
mod status;

pub use self::{
    control::{Control, ControlReader, Controller},
    intercom::{
        Intercom, IntercomMsg, IntercomReceiver, IntercomSender, IntercomStats, IntercomStatus,
        NoIntercom,
    },
    settings::{NoSettings, Settings, SettingsReader, SettingsUpdater},
    state::{NoState, State, StateHandler, StateSaver},
    stats::Stats,
    status::{Status, StatusReader, StatusUpdater},
};
use crate::watchdog::WatchdogQuery;
use async_trait::async_trait;
use futures_util::future::abortable;
use std::future::Future;
use thiserror::Error;
use tokio::{
    runtime::{Builder, Handle, Runtime},
    task::JoinHandle,
};
use tracing_futures::Instrument as _;

pub type ServiceIdentifier = &'static str;

#[async_trait]
pub trait Service: Send + Sized + 'static {
    const SERVICE_IDENTIFIER: ServiceIdentifier;

    type State: State;
    type Settings: Settings;
    type IntercomMsg: IntercomMsg;

    fn prepare(service_state: ServiceState<Self>) -> Self;

    async fn start(self);
}

pub trait ManageService {
    const SERVICE_IDENTIFIER: ServiceIdentifier;

    type State: State;
    type Settings: Settings;
    type IntercomMsg: IntercomMsg;
}

impl<T: Service> ManageService for ServiceManager<T> {
    const SERVICE_IDENTIFIER: ServiceIdentifier = T::SERVICE_IDENTIFIER;

    type State = T::State;
    type Settings = T::Settings;
    type IntercomMsg = T::IntercomMsg;
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ServiceError {
    #[error("Service cannot be started because status is: {status}")]
    CannotStart { status: Status },
}

#[derive(Debug, Clone)]
pub struct StatusReport {
    pub identifier: ServiceIdentifier,
    pub status: Status,
    pub intercom: IntercomStatus,
    pub started: u64,
}

pub struct ServiceManager<T: Service> {
    identifier: ServiceIdentifier,

    settings: SettingsUpdater<T::Settings>,
    state: StateSaver<T::State>,
    intercom_sender: IntercomSender<T::IntercomMsg>,
    intercom_stats: IntercomStats,
    started: u64,

    status: StatusReader,
    controller: Controller,
    runtime: Handle,
}

/// not to mistake for `tokio`'s runtime. This is the object that
/// will hold the service process and all the other associated data.
/// to allow for a good running activity of the service.
///
pub struct ServiceRuntime<T: Service> {
    service_state: ServiceState<T>,

    status: StatusUpdater,
    control: ControlReader,
}

/// this is the object that every services has access to
///
/// each service has its own ServiceState. It allows to connect to
/// other services [`intercom_with`] as well as getting access to the
/// service's settings or state
pub struct ServiceState<T: Service> {
    identifier: ServiceIdentifier,
    handle: Handle,
    settings: SettingsReader<T::Settings>,
    state: StateHandler<T::State>,
    intercom_receiver: IntercomReceiver<T::IntercomMsg>,
    watchdog_query: WatchdogQuery,
    status: StatusReader,
}

impl<T: Service> ServiceState<T> {
    /// access the service's Identifier
    ///
    /// this is just similar to calling `<T as Service>::SERVICE_IDENTIFIER`
    pub fn identifier(&self) -> ServiceIdentifier {
        self.identifier
    }

    /// open an [`Intercom`] handle with the given service `O`
    ///
    /// [`Intercom`]: ./struct.Intercom.html
    pub fn intercom_with<O: Service>(&self) -> Intercom<O> {
        self.watchdog_query.intercom::<O>()
    }

    /// access the `WatchdogQuery` allowing raw command access to all watchdog
    /// commands.
    pub fn watchdog_controller(&self) -> &WatchdogQuery {
        &self.watchdog_query
    }

    /// access the service's IntercomReceiver end
    ///
    /// this is the end that will receive intercom messages from other services
    pub fn intercom_mut(&mut self) -> &mut IntercomReceiver<T::IntercomMsg> {
        &mut self.intercom_receiver
    }

    /// get the [`SettingsReader`] for the given running Service
    ///
    /// this from there one can "borrow" the settings or clone the reader
    ///
    /// [`SettingsReader`]: ./struct.SettingsReader.html
    pub fn settings(&self) -> &SettingsReader<T::Settings> {
        &self.settings
    }

    /// access the status reader of the service. If the status is updated
    /// to be shutdown then the reader will receive the notification event
    /// and will be able to prepare for shutdown gracefully
    pub fn status_reader(&self) -> &StatusReader {
        &self.status
    }

    /// access the [`StateHandler`] of the running service
    ///
    /// this will allow to access or update the state of the service.
    /// Every time the state is updated, the watchdog is notified and can
    /// save a copy of the state for future uses.
    ///
    /// [`StateHandler`]: ./struct.StateHandler.html
    pub fn state(&self) -> &StateHandler<T::State> {
        &self.state
    }

    /// access the service's Runtime's handle
    ///
    /// This object can be cloned and send between tasks allowing for
    /// other tasks to create their own subtasks and so on
    pub fn runtime_handle(&self) -> &Handle {
        &self.handle
    }

    /// spawn the given future in the context of the Service's Runtime.
    ///
    /// While there is no way to enforce the users to actually spawn tasks
    /// within the Runtime we can at least urge the users to do so and avoid
    /// using the global runtime context as it may be used for other purposes.
    pub fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.runtime_handle().spawn(future)
    }
}

impl<T: Service> ServiceManager<T> {
    pub fn new<'a>(settings: &mut T::Settings, args: &clap::ArgMatches<'a>) -> (Runtime, Self) {
        let identifier = T::SERVICE_IDENTIFIER;

        settings.matches_cli_args(args);

        let mut runtime = Builder::new()
            .enable_io()
            .enable_time()
            .thread_name(identifier)
            .threaded_scheduler()
            .build()
            .unwrap();

        let settings = runtime.block_on(async { SettingsUpdater::new(settings.clone()).await });
        let state = runtime.block_on(async { StateSaver::new(T::State::default()).await });
        let status = StatusReader::new(Status::shutdown());
        let controller = runtime.block_on(async { Controller::new().await });
        let (intercom_sender, _, intercom_stats) = intercom::channel();

        let sm = Self {
            identifier,
            settings,
            state,
            intercom_sender,
            intercom_stats,
            status,
            controller,
            runtime: runtime.handle().clone(),
            started: 0,
        };

        (runtime, sm)
    }

    pub fn intercom(&self) -> IntercomSender<T::IntercomMsg> {
        self.intercom_sender.clone()
    }

    pub async fn status(&self) -> StatusReport {
        StatusReport {
            identifier: self.identifier,
            status: self.status.status(),
            intercom: self.intercom_stats.status().await,
            started: self.started,
        }
    }

    pub fn shutdown(&mut self) {
        match self.status.status() {
            Status::Shutdown { .. } | Status::ShuttingDown { .. } => {
                // Ignore as the node is either shutdown or already shutting
                // down
            }
            Status::Starting { .. } | Status::Started { .. } => {
                // send only if the node will have a chance to actually read
                // the command
                self.controller.send(Control::Shutdown)
            }
        }
    }

    pub fn runtime(
        &mut self,
        watchdog_query: WatchdogQuery,
    ) -> Result<ServiceRuntime<T>, ServiceError> {
        let status = self.status.status();
        if !status.is_shutdown() {
            Err(ServiceError::CannotStart { status })
        } else {
            let (intercom_sender, intercom_receiver, intercom_stats) =
                intercom::channel::<T::IntercomMsg>();

            std::mem::replace(&mut self.intercom_sender, intercom_sender);
            std::mem::replace(&mut self.intercom_stats, intercom_stats);

            self.started += 1;

            Ok(ServiceRuntime {
                service_state: ServiceState {
                    identifier: self.identifier,
                    handle: self.runtime.clone(),
                    settings: self.settings.reader(),
                    state: self.state.handler(),
                    status: self.status.clone(),
                    intercom_receiver,
                    watchdog_query,
                },
                status: self.status.updater(),
                control: self.controller.reader(),
            })
        }
    }
}

impl<T: Service> ServiceRuntime<T> {
    pub fn start(self) {
        let ServiceRuntime {
            service_state,
            status,
            mut control,
        } = self;

        let service_identifier: &'static str = service_state.identifier;

        status.update(Status::starting());

        let watchdog_query = service_state.watchdog_query.clone();
        let handle = service_state.handle.clone();
        let runner = T::prepare(service_state);

        let (runner, abort_handle) = abortable(async move {
            let span = tracing::info_span!("service", service_identifier);
            let _enter = span.enter();

            runner.start().in_current_span().await
        });

        let mut service_join_handle = handle.spawn(runner);

        // the runner (the service) has been started into its current runtime. They must use
        // the `handle` to spawn new tasks.
        //
        // however the control of the service is still spawned in the watchdog current context
        // so we can perform the management tasks without disrupting the service's runtime
        watchdog_query.spawn(async move {
            status.update(Status::started());

            let span = tracing::debug_span!("service control", service_identifier);
            let _enter = span.enter();

            loop {
                tokio::select! {
                    join_result = &mut service_join_handle => {
                        if let Err(join_error) = join_result {
                            // TODO: the task could not join, either cancelled
                            //       or panicked. Ideally we need to document
                            //       this panic and see what kind of strategy
                            //       can be applied (can we restart the service?)
                            //       or is it a fatal panic and we cannot recover?

                            tracing::error!(
                                "main process failed with following error: {:#?}",
                                join_error
                            );
                        } else {
                            // nothing to do her, the service already finished and
                            // returned successfully
                        }
                        status.update(Status::shutdown());
                        break;
                    }
                    control = control.updated() => {
                        match control {
                            Some(Control::Shutdown) => {
                                tracing::info!("shutting down...");

                                // updating the status will notify the `StatusReader` in the `ServiceState`
                                // if watched, the future will yield and the service will be able to prepare
                                // for the service shutdown and exit gracefully.
                                status.update(Status::shutting_down());
                            }
                            None | Some(Control::Kill) => {
                                tracing::info!("Terminating...");
                                status.update(Status::shutdown());
                                abort_handle.abort();
                                break;
                            }
                        }
                    }
                };
            }
        });
    }
}

impl<T: Service> Drop for ServiceManager<T> {
    fn drop(&mut self) {
        if !self.status.status().is_shutdown() {
            self.controller.send(Control::Kill)
        }
    }
}
