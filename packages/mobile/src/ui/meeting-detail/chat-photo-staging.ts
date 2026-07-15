// Pure staging model for photos attached to the next chat message.
// Kept free of expo/RN imports so the node-env vitest runner can
// exercise the add/remove/limit logic; the camera capture + upload
// glue lives in PhotoAttachButton / the MeetingScreen send handler.

/// A photo the user staged for the next chat send. `uri` is a local
/// (already-downscaled) file:// path; `mime` is the upload
/// Content-Type. `id` is a client-minted key for list rendering +
/// removal (the server assigns the real attachment id on upload).
export interface StagedPhoto {
  id: string;
  uri: string;
  mime: string;
}

/// Mirrors the Mac's chatAttachmentLimit — keeps the vision prompt
/// legible and uploads bounded at four images per turn.
export const CHAT_PHOTO_LIMIT = 4;

export function canAddPhoto(list: StagedPhoto[]): boolean {
  return list.length < CHAT_PHOTO_LIMIT;
}

/// Append a photo if under the limit; a no-op (returns the same list)
/// once four are staged, so callers can gate the button on
/// canAddPhoto without a second guard.
export function addPhoto(list: StagedPhoto[], photo: StagedPhoto): StagedPhoto[] {
  if (!canAddPhoto(list)) return list;
  return [...list, photo];
}

export function removePhoto(list: StagedPhoto[], id: string): StagedPhoto[] {
  return list.filter((p) => p.id !== id);
}
