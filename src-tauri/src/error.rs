use serde::Serialize;
use thiserror::Error;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("输入错误: {0}")]
    InvalidInput(String),
    #[error("资源不存在: {0}")]
    NotFound(String),
    #[error("状态冲突: {0}")]
    Conflict(String),
    #[error("文件操作失败: {0}")]
    Io(String),
    #[error("订阅请求失败: {0}")]
    Subscription(String),
    #[error("应用更新失败: {0}")]
    Update(String),
    #[error("配置处理失败: {0}")]
    Config(String),
    #[error("Mihomo 运行失败: {0}")]
    Runtime(String),
    #[error("网络预检失败: {message}")]
    NetworkPreflight { message: String, retryable: bool },
    #[error("系统网络设置失败: {0}")]
    Platform(String),
}

impl AppError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidInput(_) => "INVALID_INPUT",
            Self::NotFound(_) => "NOT_FOUND",
            Self::Conflict(_) => "STATE_CONFLICT",
            Self::Io(_) => "IO_ERROR",
            Self::Subscription(_) => "SUBSCRIPTION_ERROR",
            Self::Update(_) => "UPDATE_ERROR",
            Self::Config(_) => "CONFIG_ERROR",
            Self::Runtime(_) => "CORE_ERROR",
            Self::NetworkPreflight { .. } => "NETWORK_CHECK_FAILED",
            Self::Platform(_) => "PLATFORM_ERROR",
        }
    }

    pub fn stage(&self) -> &'static str {
        match self {
            Self::InvalidInput(_) => "input",
            Self::NotFound(_) | Self::Io(_) => "storage",
            Self::Conflict(_) => "state",
            Self::Subscription(_) => "subscription",
            Self::Update(_) => "update",
            Self::Config(_) => "config",
            Self::Runtime(_) => "runtime",
            Self::NetworkPreflight { .. } => "network",
            Self::Platform(_) => "platform",
        }
    }

    pub fn retryable(&self) -> bool {
        if let Self::NetworkPreflight { retryable, .. } = self {
            return *retryable;
        }
        matches!(
            self,
            Self::Subscription(_) | Self::Update(_) | Self::Runtime(_) | Self::Platform(_)
        )
    }

    pub fn dto(&self) -> AppErrorDto {
        AppErrorDto {
            code: self.code().to_string(),
            stage: self.stage().to_string(),
            message: self.to_string(),
            retryable: self.retryable(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppErrorDto {
    pub code: String,
    pub stage: String,
    pub message: String,
    pub retryable: bool,
}

impl From<std::io::Error> for AppError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}
