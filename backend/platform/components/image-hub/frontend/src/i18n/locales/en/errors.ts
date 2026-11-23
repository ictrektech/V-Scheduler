/**
 * Error code namespace (en): image-hub backend codes 20801–20812 and unknown fallbacks.
 */
export default {
  errors: {
    20801: 'Invalid parameters: check the image name, tag and request fields',
    20802: 'Cannot reach the local registry service',
    20803: 'The target image, repository or tag does not exist',
    20804: 'Image deletion failed (the registry may not have deletion enabled)',
    20805: 'Platform API call failed; please try again later',
    20806: 'Local file read/write failed; check the data directory',
    20807: 'Image transfer failed: check the upstream address, network connectivity or mirror',
    20808: 'The task does not exist or has been cleaned up',
    20809: 'Invalid image reference: check the image name, tag or architecture format',
    20810: 'The task was cancelled',
    20811: 'Registry garbage collection failed; see the task logs',
    20812: 'Registry garbage collection is unavailable',
    unknown: 'Operation failed. Please try again later.',
    unknownWithCode: 'Operation failed (error code {code}). Please try again later.',
  },
};