/** Saves `text` as a file through the browser's download. */
export function saveText(filename: string, text: string) {
  const url = URL.createObjectURL(new Blob([text], { type: "text/plain;charset=utf-8" }));
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  document.body.append(a);
  a.click();
  a.remove();
  // Revoked later: some browsers start the download after click() returns.
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}
