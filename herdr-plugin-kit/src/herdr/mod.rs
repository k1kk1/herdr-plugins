mod api;
mod client;
mod model;

pub use client::{api_error, socket_path, ApiError, Herdr};
pub use model::{
    Agent, AgentStatus, Direction, InstalledPlugin, Layout, LayoutNode, MoveResult, Pane, PluginAction,
    Tab, Workspace,
};
