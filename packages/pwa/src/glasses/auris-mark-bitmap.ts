//! Render the Auris brand mark as PNG bytes suitable for
//! `bridge.updateImageRawData(...)`.
//!
//! Despite the SDK doc's "4-bit greyscale" framing, the host expects
//! a **standard image format** (PNG / JPEG) on the wire — it runs
//! image-format detection then dithers to 4-bit greyscale on its
//! end. Sending raw packed-nibble pixel data fails with `sendFailed`
//! because the simulator's first step is `image::guess_format(bytes)`
//! and our nibble buffer has no magic number.
//!
//! Geometry is lifted from `assets/branding/icon-*.svg` (and matches
//! the SVG/RN `AurisMark` we render everywhere else):
//!
//!   Original viewBox  96 × 96
//!   Mark group        translated by (34, 28)
//!   Outer arc         M 22  4  A 18 18 0 0 0 22 40   stroke 4.5  round caps
//!   Inner arc         M 22 12  A 10 10 0 0 0 22 32   stroke 4.5  round caps
//!   Focal dot         cx 16  cy 22  r 3

import { LOGO_CONTAINER_ID, LOGO_CONTAINER_NAME, LOGO_SIZE } from "./layout-unpaired";

const ORIG_VIEWBOX = 96;

interface ImageBridge {
  updateImageRawData(data: unknown): Promise<unknown>;
}

/// Paint the Auris brand mark into the unpaired layout's logo
/// container. Used at boot (right after `createStartUpPageContainer`)
/// and again after the re-pair flow rebuilds the unpaired layout —
/// image containers are placeholders after rebuild, so the pixels
/// have to be re-uploaded.
///
/// Failures degrade to a logoless splash rather than blocking the
/// caller: jsdom doesn't have a Canvas2D context (so unit tests
/// throw inside `drawAurisMarkPng`), and a single bad upload on real
/// glasses shouldn't keep the user from pairing.
export async function paintAurisMark(bridge: ImageBridge): Promise<void> {
  try {
    const png = await drawAurisMarkPng(LOGO_SIZE);
    const result = await bridge.updateImageRawData({
      containerID: LOGO_CONTAINER_ID,
      containerName: LOGO_CONTAINER_NAME,
      imageData: Array.from(png),
    });
    if (String(result) !== "success") {
      console.warn(
        `[unpaired] updateImageRawData returned ${String(result)} ` +
          `(png size: ${png.byteLength}B). Splash will render without the logo.`,
      );
    }
  } catch (e) {
    console.warn("[unpaired] paintAurisMark threw:", e);
  }
}

/// Build a square `size × size` PNG of the Auris mark, returned as
/// a `Uint8Array` of the PNG bytes. Wraps Canvas2D drawing + a
/// `toBlob('image/png')` round-trip — the bytes are ready to drop
/// into `imageData` (as `Array.from(...)` or base64 — both work).
export async function drawAurisMarkPng(size: number): Promise<Uint8Array> {
  const canvas = document.createElement("canvas");
  canvas.width = size;
  canvas.height = size;
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("2D context unavailable — glasses bitmap can't be drawn");

  // We draw on transparent and rely on the SDK's host-side dither to
  // map alpha into the 4-bit greyscale levels. Empty pixels stay
  // transparent in the PNG; the glasses' canvas has no background
  // (unpainted areas pass through to the real world).
  const scale = size / ORIG_VIEWBOX;
  ctx.save();
  ctx.scale(scale, scale);
  ctx.translate(34, 28);

  ctx.strokeStyle = "white";
  ctx.fillStyle = "white";
  ctx.lineWidth = 4.5;
  ctx.lineCap = "round";

  // Outer arc — the helix of the ear. Both endpoints sit at x=22 so
  // the chord is a vertical diameter; sweep CCW (anticlockwise=true)
  // makes it bulge to the LEFT.
  ctx.beginPath();
  ctx.arc(22, 22, 18, -Math.PI / 2, Math.PI / 2, true);
  ctx.stroke();

  // Inner arc — concentric with the outer arc but smaller.
  ctx.beginPath();
  ctx.arc(22, 22, 10, -Math.PI / 2, Math.PI / 2, true);
  ctx.stroke();

  // Focal dot — sits where the tragus would be, slightly inside the
  // inner arc.
  ctx.beginPath();
  ctx.arc(16, 22, 3, 0, Math.PI * 2);
  ctx.fill();

  ctx.restore();

  const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/png"));
  if (!blob) throw new Error("canvas.toBlob returned null — PNG encode failed");
  return new Uint8Array(await blob.arrayBuffer());
}
