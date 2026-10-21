// One call: an 8-bit RGB image as a lossless JPEG XL codestream (libjxl's encoder, no thread pool).
#include <stdlib.h>
#include <jxl/encode.h>

int scenic_jxl_lossless_rgb8(const unsigned char *rgb, unsigned w, unsigned h, int effort,
                             unsigned char **out, size_t *len) {
  JxlEncoder *enc = JxlEncoderCreate(NULL);
  if (!enc) return 1;
  int rc = 2;
  JxlBasicInfo info;
  JxlEncoderInitBasicInfo(&info);
  info.xsize = w;
  info.ysize = h;
  info.bits_per_sample = 8;
  info.num_color_channels = 3;
  info.uses_original_profile = JXL_TRUE;
  JxlColorEncoding color;
  JxlColorEncodingSetToSRGB(&color, JXL_FALSE);
  JxlPixelFormat pf = {3, JXL_TYPE_UINT8, JXL_NATIVE_ENDIAN, 0};
  JxlEncoderFrameSettings *fs = NULL;
  size_t cap = 65536, used = 0;
  unsigned char *buf = NULL;
  if (JxlEncoderSetBasicInfo(enc, &info) != JXL_ENC_SUCCESS) goto done;
  if (JxlEncoderSetColorEncoding(enc, &color) != JXL_ENC_SUCCESS) goto done;
  fs = JxlEncoderFrameSettingsCreate(enc, NULL);
  if (JxlEncoderSetFrameLossless(fs, JXL_TRUE) != JXL_ENC_SUCCESS) goto done;
  if (JxlEncoderFrameSettingsSetOption(fs, JXL_ENC_FRAME_SETTING_EFFORT, effort) != JXL_ENC_SUCCESS) goto done;
  if (JxlEncoderAddImageFrame(fs, &pf, rgb, (size_t)w * h * 3) != JXL_ENC_SUCCESS) goto done;
  JxlEncoderCloseInput(enc);
  buf = malloc(cap);
  if (!buf) goto done;
  for (;;) {
    unsigned char *next = buf + used;
    size_t avail = cap - used;
    JxlEncoderStatus s = JxlEncoderProcessOutput(enc, &next, &avail);
    used = (size_t)(next - buf);
    if (s == JXL_ENC_SUCCESS) break;
    if (s != JXL_ENC_NEED_MORE_OUTPUT) goto done;
    unsigned char *grown = realloc(buf, cap * 2);
    if (!grown) goto done;
    buf = grown;
    cap *= 2;
  }
  *out = buf;
  *len = used;
  buf = NULL;
  rc = 0;
done:
  free(buf);
  JxlEncoderDestroy(enc);
  return rc;
}
