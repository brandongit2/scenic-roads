// What the dev tools app's parts share to answer HTTP: a refusal (status, code, why, each field's
// reason), JSON and file answers, and request bodies read with a bound.

export class Refusal extends Error {
  constructor(status, code, message, errors) { super(message); Object.assign(this, { status, code, errors }); }
}
export const refuse = (status, code, message, errors) => { throw new Refusal(status, code, message, errors); };

export const send = (res, code, body, type = "application/json; charset=utf-8", headers = {}) => {
  res.writeHead(code, { "content-type": type, "cache-control": "no-store", ...headers });
  res.end(typeof body === "string" || Buffer.isBuffer(body) ? body : JSON.stringify(body));
};
export const fail = (res, code, errCode, message, errors) => send(res, code, { error: { code: errCode, message, ...(errors ? { errors } : {}) } });

// The body, refused past `max` bytes (`{code: "too_large", max}`).
export const readBody = (req, max) => new Promise((resolve, reject) => {
  const chunks = []; let n = 0;
  req.on("data", (c) => { n += c.length; if (n > max) { reject({ code: "too_large", max }); req.destroy(); } else chunks.push(c); });
  req.on("end", () => resolve(Buffer.concat(chunks)));
  req.on("error", reject);
});
export const jsonBody = async (req, max = 2e6) => {
  const text = (await readBody(req, max)).toString("utf8");
  try { return JSON.parse(text || "{}"); } catch { return refuse(400, "invalid_argument", "the body is not valid JSON"); }
};
