import { ChatError } from './request.mjs';

export class UnsupportedFinishReasonError extends ChatError {
  code = 'ROSTER_LLM_FINISH_REASON';

  constructor(reason, { truncated = reason === 'length' } = {}) {
    super(`The LLM response had an unsupported finish reason: ${reason}.`);
    this.finishReason = reason;
    this.truncated = truncated;
  }
}
