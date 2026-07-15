import { describe, expect, it } from "vitest";

import {
  addPhoto,
  canAddPhoto,
  CHAT_PHOTO_LIMIT,
  removePhoto,
  type StagedPhoto,
} from "./chat-photo-staging";

const photo = (id: string): StagedPhoto => ({
  id,
  uri: `file://${id}.jpg`,
  mime: "image/jpeg",
});

describe("chat photo staging", () => {
  it("appends a photo", () => {
    expect(addPhoto([], photo("a"))).toEqual([photo("a")]);
  });

  it("removes by id", () => {
    expect(removePhoto([photo("a"), photo("b")], "a")).toEqual([photo("b")]);
  });

  it("caps at the limit and drops further adds", () => {
    let list: StagedPhoto[] = [];
    for (let i = 0; i < CHAT_PHOTO_LIMIT + 2; i++) list = addPhoto(list, photo(`p${i}`));
    expect(list).toHaveLength(CHAT_PHOTO_LIMIT);
    expect(canAddPhoto(list)).toBe(false);
  });

  it("allows adding again after a removal frees a slot", () => {
    let list: StagedPhoto[] = [];
    for (let i = 0; i < CHAT_PHOTO_LIMIT; i++) list = addPhoto(list, photo(`p${i}`));
    list = removePhoto(list, "p0");
    expect(canAddPhoto(list)).toBe(true);
    list = addPhoto(list, photo("new"));
    expect(list).toHaveLength(CHAT_PHOTO_LIMIT);
  });
});
