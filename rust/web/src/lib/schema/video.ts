// Settings for video, encoders, AMD AMF and NVIDIA NVENC.
// Generated from rust/core/config/settings.toml by tools/generated_docs.py.
import type { Setting } from '../settings-types';
import { settings as stream } from './video_stream.gen';
import { settings as encoders } from './video_encoders.gen';

export const settings: Setting[] = [...stream, ...encoders];
