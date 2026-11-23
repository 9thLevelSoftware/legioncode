//! Milestone contracts, in-memory replay/stream hub, SEP-3415 event descriptors,
//! and orchestration-to-milestone projection (`apps/server/src/milestones/`).

mod event_enum;
mod events;
mod hub;
mod orchestration;
mod orchestration_helpers;
mod stream;
mod time;
mod types;

pub use event_enum::{MilestoneEvent, milestone_matches};
pub use events::{
    MILESTONE_DEFAULT_EVENTS, MILESTONE_MAX_EVENTS, McpEventStreamItem, MilestoneEventCategory,
    MilestoneEventDescriptor, MilestoneEventPollPage, MilestoneEventPollRequest, MilestoneEvents,
    MilestoneOccurrence, MilestoneThreadFilterArgs, MilestoneWorkflowFilterArgs,
    hidden_from_threads,
};
pub use hub::{
    MILESTONE_IDS_KEPT, MILESTONE_KEPT, MILESTONE_RECENT_LIMIT, MILESTONE_SUBSCRIBER_BACKLOG,
    MilestoneHub, MilestoneReadOptions, MilestoneReadPage, MilestoneRecentOptions,
    MilestoneRecentPage,
};
pub use orchestration::{OrchestrationMilestones, WorkflowMilestoneMeta};
pub use stream::{
    MilestoneFollowItem, MilestoneFollowStream, MilestoneSubscription, PositionedMilestone,
};
pub use time::{format_base36, parse_iso_epoch_ms};
pub use types::{
    MilestoneActor, MilestoneActorType, MilestoneEnvelope, MilestoneEventType, MilestoneFilter,
    MilestoneGuestResolution, MilestoneId, MilestoneIdError, MilestoneInputKind,
    MilestoneInputResolution, MilestonePrOutcome, MilestoneRunOutcome,
    MilestoneWorkflowInputResolution, MilestoneWorkflowOutcome, WorkflowRunFields,
};
