import PERSONA_RAW from "../PERSONA.md";
import MODEL_PIN_RAW from "../../../MODEL_PIN";
import BRIEF from "../brief-fields.json";
import {createSecretary, createTelegramWebhook, createVisitorSessionBase}
  from "./secretary-core.js";

const MODEL_PIN = MODEL_PIN_RAW.split("#", 1)[0].trim();
if (!MODEL_PIN) throw new Error("MODEL_PIN is empty — refusing to serve on an unpinned model");

const RUNTIME = createSecretary({persona: PERSONA_RAW.trim(), modelPin: MODEL_PIN, brief: BRIEF});
const VisitorSessionBase = createVisitorSessionBase(RUNTIME);

export class VisitorSession extends VisitorSessionBase {}

export default createTelegramWebhook();
