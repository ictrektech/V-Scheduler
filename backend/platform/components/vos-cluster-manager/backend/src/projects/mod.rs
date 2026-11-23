//! 平台 projects API 客户端（/v1000/app/projects/* 编排，D-VCM-03/08）

pub mod access_token;
pub mod client;
pub mod dto;

pub use client::{APP_ID,

    MockProjectsClient, ProjectsClient, RealProjectsClient, UnavailableProjectsClient,
};
pub use dto::{
    ProjectDeployBo, ProjectDetailVo, ProjectInstanceVo, ProjectRemoveBo, ProjectRuntimeVo,
};