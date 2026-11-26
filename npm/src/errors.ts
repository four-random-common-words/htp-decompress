export type HtpDecompressionErrorCode =
  | 'truncated'
  | 'corrupt'
  | 'unsupported'
  | 'memory-limit'
  | 'trailing-data'
  | 'internal';

export class HtpDecompressionError extends TypeError {
  override readonly name = 'HtpDecompressionError';
  readonly code: HtpDecompressionErrorCode;

  constructor(code: HtpDecompressionErrorCode, message: string, options?: ErrorOptions) {
    super(message, options);
    this.code = code;
  }
}
