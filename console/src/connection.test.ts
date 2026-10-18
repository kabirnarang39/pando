import { describe, expect, it } from "vitest";
import { sessionFromHash } from "./connection";

describe("the session handed over after signing in", () => {
  it("is read from the fragment", () => {
    expect(sessionFromHash("#pando_session=pando_0123abcd")).toBe("pando_0123abcd");
  });
  it("is ignored unless it is exactly a Pando token", () => {
    for (const h of ["", "#/", "#/lumen", "#pando_session=", "#pando_session=evil", "#pando_session=pando_a&x=1", "#x=1&pando_session=pando_a", "#pando_session=pando_<b>"]) {
      expect(sessionFromHash(h), h).toBeNull();
    }
  });
});
