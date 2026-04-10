#pragma once

#if __has_include(<streamcamera/streamcamera.h>)

#include <streamcamera/streamcamera.h>

#else

#include <cstdint>
#include <memory>
#include <string>
#include <vector>

namespace Aurora::StreamCamera {

enum class CameraFacing {
    Unknown,
    Front,
    Rear,
    Screen,
};

enum class CameraCapabilityPriority {
    WidthHeightFps,
    HeightWidthFps,
    FpsWidthHeight,
    FpsHeightWidth,
};

enum class PixelFormat : uint32_t {
    Invalid = 0,
    YUV420Planar = 1,
    YUV420SemiPlanar = 2,
    YCbCrFlexible = 0xff,
};

enum class HandleType {
    NoHandle,
    ANativeWindowBuffer,
    EGL = ANativeWindowBuffer,
    GBMImportData,
};

enum class CameraParameter : unsigned int {
    FlashMode,
    Last,
    Invalid,
};

struct CameraCapability {
    unsigned int width = 0;
    unsigned int height = 0;
    unsigned int fps = 0;
};

struct CameraCapabilityEx {
    unsigned int width = 0;
    unsigned int height = 0;
    unsigned int fps = 0;
};

struct CameraCapabilityRanges {
};

struct PixelFormatDescription {
};

struct CameraInfo {
    std::string id;
    std::string name;
    std::string provider;
    CameraFacing facing = CameraFacing::Unknown;
    unsigned int mountAngle = 0;
    std::string metadata;
};

struct YCbCrFrame {
    const uint8_t *y = nullptr;
    const uint8_t *cb = nullptr;
    const uint8_t *cr = nullptr;
    uint16_t yStride = 0;
    uint16_t cStride = 0;
    uint16_t chromaStep = 0;
    uint16_t width = 0;
    uint16_t height = 0;
    uint64_t timestampUs = 0;
};

struct RawImageFrame {
};

struct Frame {
};

class GraphicBuffer
{
public:
    virtual ~GraphicBuffer() = default;

    virtual std::shared_ptr<const YCbCrFrame> mapYCbCr() = 0;
    virtual std::shared_ptr<const RawImageFrame> map() = 0;
    virtual std::shared_ptr<const Frame> mapFrame() = 0;
    virtual uint16_t rotation() const = 0;

    uint16_t width = 0;
    uint16_t height = 0;
    uint64_t timestampUs = static_cast<uint64_t>(-1);
    PixelFormat pixelFormat = PixelFormat::Invalid;
    const void *handle = nullptr;
    HandleType handleType = HandleType::NoHandle;
};

class CameraListener
{
public:
    virtual ~CameraListener() = default;

    virtual void onCameraFrame(std::shared_ptr<GraphicBuffer> buffer) = 0;
    virtual void onCameraError(const std::string &errorDescription) = 0;
    virtual void onCameraParameterChanged(CameraParameter param, const std::string &value) = 0;
};

class Camera
{
public:
    virtual ~Camera() = default;

    virtual bool getInfo(CameraInfo &info) = 0;
    virtual std::vector<PixelFormat> getSupportedPixelFormats() = 0;
    virtual bool startCapture(const CameraCapability &cap, PixelFormat format = PixelFormat::YCbCrFlexible) = 0;
    virtual bool stopCapture() = 0;
    virtual bool captureStarted() const = 0;
    virtual std::string getParameterRange(CameraParameter param) = 0;
    virtual std::string getParameter(CameraParameter param) = 0;
    virtual bool setParameter(CameraParameter param, const std::string &value) = 0;
    virtual void setListener(CameraListener *listener) = 0;
    virtual std::vector<PixelFormatDescription> getSupportedFormats() = 0;
    virtual bool isFormatSupported(const PixelFormatDescription &formatDesc) = 0;
    virtual bool isFormatSupported(PixelFormat pixelFormat) = 0;
    virtual bool queryCapabilityRanges(PixelFormat format, CameraCapabilityRanges &capRanges) = 0;
    virtual bool findClosestCapability(PixelFormat format,
                                       const CameraCapabilityEx &desired,
                                       CameraCapabilityEx &found,
                                       CameraCapabilityPriority priorityHint = CameraCapabilityPriority::WidthHeightFps) = 0;
    virtual bool startCapture(const CameraCapabilityEx &cap,
                              PixelFormat format = PixelFormat::YCbCrFlexible) = 0;
    virtual bool queryCapabilities(std::vector<CameraCapability> &caps) = 0;
};

class CameraManagerListener;

class CameraManager
{
public:
    virtual ~CameraManager() = default;

    virtual bool init() = 0;
    virtual int getNumberOfCameras() = 0;
    virtual bool getCameraInfo(unsigned int num, CameraInfo &info) = 0;
    virtual bool queryCapabilities(const std::string &cameraId, std::vector<CameraCapability> &caps) = 0;
    virtual std::shared_ptr<Camera> openCamera(const std::string &cameraId) = 0;
    virtual std::vector<PixelFormatDescription> getSupportedFormats(const std::string &cameraId) = 0;
    virtual bool queryCapabilityRanges(const std::string &cameraId,
                                       PixelFormat format,
                                       CameraCapabilityRanges &capRanges) = 0;
    virtual bool findClosestCapability(const std::string &cameraId,
                                       PixelFormat format,
                                       const CameraCapabilityEx &desired,
                                       CameraCapabilityEx &found,
                                       CameraCapabilityPriority priorityHint = CameraCapabilityPriority::WidthHeightFps) = 0;
    virtual void setListener(CameraManagerListener *listener) = 0;
};

} // namespace Aurora::StreamCamera

extern "C" Aurora::StreamCamera::CameraManager *StreamCameraManager();

#endif
