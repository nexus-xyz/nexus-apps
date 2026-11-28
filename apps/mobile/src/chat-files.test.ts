import type { ChatAttachmentRef } from "@nexus/kit";
import { describe, expect, it } from "vitest";
import { isDataFile, resend } from "./chat-files";
import { isImage } from "./chat-images";

const png: ChatAttachmentRef = { id: "cha_1", type: "image", mediaType: "image/png", bytes: 2, name: "chart.png" };
const csv: ChatAttachmentRef = { id: "chf_1", type: "file", mediaType: "text/csv", bytes: 8, name: "sales.csv" };

describe("a question's data files on Retry", () => {
  it("sends a data file again as a file, never as an image", () => {
    expect([png, csv].filter(isImage)).toEqual([png]);
    expect([png, csv].filter(isDataFile)).toEqual([csv]);
  });

  it("sends a data file under its own name, which a page reads it by", () => {
    expect(resend(csv, { mediaType: "text/csv", data: "YSxiCjEsMg==" })).toEqual({ ref: csv, input: { data: "data:text/csv;base64,YSxiCjEsMg==", name: "sales.csv" } });
    const unnamed: ChatAttachmentRef = { ...csv, name: undefined, mediaType: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" };
    expect(resend(unnamed, { mediaType: unnamed.mediaType, data: "UEs=" }).input.name).toBe("file.xlsx");
  });
});
