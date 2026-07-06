// Public surface for the contract TS package. Each generated .ts
// file re-exports its own ts-proto helper symbols (`DeepPartial`,
// `MessageFns`, `protobufPackage`), so a wildcard re-export across
// all three would collide. We list message types explicitly here
// instead — consumers get a flat namespace and the per-file
// helpers stay scoped to their generated module.
//
// `Error` and `Event` are intentionally re-exported under their
// proto names; consumers shadowing the JS built-in / DOM type can
// rename at import: `import { Event as WireEvent } from "..."`.

// ─── Common types (common.proto) ────────────────────────────────────────
export {
  MeetingState,
  meetingStateFromJSON,
  meetingStateToJSON,
  UpdateStrategy,
  updateStrategyFromJSON,
  updateStrategyToJSON,
  Capability,
  capabilityFromJSON,
  capabilityToJSON,
  ModeOption,
  Item,
  Status,
  PriorContextSummary,
  Device,
} from "./gen/auris/v1/common.js";

// ─── Intents (intents.proto) ────────────────────────────────────────────
export {
  Intent,
  StartMeeting,
  StopMeeting,
  SetMode,
  SetMetadata,
  RegisterDevice,
  MarkMoment,
  ExpandItem,
  Chat,
  CancelChat,
  UpsertQuickAsk,
  DeleteQuickAsk,
} from "./gen/auris/v1/intents.js";

// ─── Events (events.proto) ──────────────────────────────────────────────
export {
  Event,
  Snapshot,
  MeetingStateChanged,
  ModeChanged,
  DisplayTagChanged,
  MetadataChanged,
  PriorContextChanged,
  ItemsUpdate,
  ItemUpdated,
  TranscriptInterim,
  StatusEvent,
  Error,
  DeviceRegistered,
  DevicesChanged,
  AudioSourceDeviceChanged,
  ArtifactsChanged,
  CaptureMomentScreenshot,
  MomentSummarized,
} from "./gen/auris/v1/events.js";

/**
 * The wire-protocol version this package's generated types speak.
 * Mirrors `Snapshot.protocolVersion` for callers that need the
 * constant outside an event payload.
 */
export const PROTOCOL_VERSION = 1 as const;
