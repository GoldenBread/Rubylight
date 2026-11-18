// MetalFX operates independently of SDL. Only IOSurfaces cross the boundary.
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#import <MetalFX/MetalFX.h>
#import <QuartzCore/CAMetalLayer.h>
#import <CoreVideo/CoreVideo.h>
#import <IOSurface/IOSurfaceRef.h>

// Output images SDL draws. One is reused only when nothing but this ring holds it and its
// IOSurface's use count is back to what the ring's own texture alone gives it: a Metal texture of
// the surface (SDL's, kept alive by an in-flight command buffer too) raises that count while it
// exists. Otherwise the next is tried, and a frame with all of them busy gets an image of its own.
#define NJ_RING 4

@interface NJSpatial : NSObject {
@public
    CVPixelBufferRef ring[NJ_RING];
    int32_t idle_use[NJ_RING];
    __strong id<MTLTexture> targets[NJ_RING];
    int next;
}
@property id<MTLDevice> device;
@property id<MTLCommandQueue> queue;
@property id<MTLComputePipelineState> convert;
@property id<MTLFXSpatialScaler> scaler;
@property id<MTLTexture> rgb;
@property id<MTLTexture> scaled;
@property CVMetalTextureCacheRef cache;
@end
@implementation NJSpatial
- (void)dealloc {
    if (_cache) CFRelease(_cache);
    for (int i=0;i<NJ_RING;i++) if (ring[i]) CFRelease(ring[i]);
}
@end

// A BGRA IOSurface image the scaler writes and SDL's Metal renderer reads.
static bool nj_new_output(NJSpatial *s, CVPixelBufferRef *buffer, id<MTLTexture> __strong *texture) {
    CVPixelBufferRef b=NULL;
    NSDictionary *attributes=@{(id)kCVPixelBufferIOSurfacePropertiesKey:@{},(id)kCVPixelBufferMetalCompatibilityKey:@YES};
    if (CVPixelBufferCreate(NULL,s.scaler.outputWidth,s.scaler.outputHeight,kCVPixelFormatType_32BGRA,(__bridge CFDictionaryRef)attributes,&b)!=kCVReturnSuccess) return false;
    MTLTextureDescriptor *d=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatBGRA8Unorm width:s.scaler.outputWidth height:s.scaler.outputHeight mipmapped:NO];
    d.usage=MTLTextureUsageShaderRead;
    id<MTLTexture> t=[s.device newTextureWithDescriptor:d iosurface:CVPixelBufferGetIOSurface(b) plane:0];
    if (!t) { CFRelease(b); return false; }
    *buffer=b; *texture=t;
    return true;
}

bool nj_metalfx_supported(void) {
    @autoreleasepool {
        if (@available(macOS 13.0, *)) {
            id<MTLDevice> device=MTLCreateSystemDefaultDevice();
            return device && [MTLFXSpatialScalerDescriptor supportsDevice:device];
        }
        return false;
    }
}

// The conversion kernel is the same for every size and scaler, so it is compiled once per device:
// a window resize builds a new scaler, and newLibraryWithSource there compiled it again on the
// presenting thread.
static id<MTLComputePipelineState> nj_conversion_pipeline(id<MTLDevice> device) {
    static NSMutableDictionary<NSNumber *, id<MTLComputePipelineState>> *pipelines;
    static NSLock *lock;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        pipelines=[NSMutableDictionary new];
        lock=[NSLock new];
    });
    NSNumber *key=@(device.registryID);
    [lock lock];
    id<MTLComputePipelineState> pipeline=pipelines[key];
    [lock unlock];
    if (pipeline) return pipeline;
    // Inputs are normalized NV12/P010. Range and matrix come from AVFrame, not host brand.
    NSString *source = @"#include <metal_stdlib>\nusing namespace metal;\n"
    "kernel void yuv(texture2d<float, access::sample> y [[texture(0)]], texture2d<float, access::sample> uv [[texture(1)]], texture2d<float, access::write> rgb [[texture(2)]], constant float4 *p [[buffer(0)]], uint2 xy [[thread_position_in_grid]]) {"
    "if (xy.x >= rgb.get_width() || xy.y >= rgb.get_height()) return;"
    "constexpr sampler sm(coord::normalized, address::clamp_to_edge, filter::linear);"
    "float2 pos=(float2(xy)+0.5)/float2(y.get_width(),y.get_height());"
    "float l=(y.sample(sm,pos).r-p[0].x)*p[0].y; float2 c=(uv.sample(sm,pos).rg-p[0].z)*p[0].w;"
    "rgb.write(float4(clamp(float3(l+p[1].x*c.y,l+p[1].y*c.x+p[1].z*c.y,l+p[1].w*c.x),0.0,1.0),1),xy);}";
    NSError *error = nil;
    id<MTLLibrary> library = [device newLibraryWithSource:source options:nil error:&error];
    if (!library) { NSLog(@"Nightjar MetalFX conversion: %@", error); return nil; }
    pipeline = [device newComputePipelineStateWithFunction:[library newFunctionWithName:@"yuv"] error:&error];
    if (!pipeline) { NSLog(@"Nightjar MetalFX conversion: %@", error); return nil; }
    [lock lock];
    // Another stream may have built it meanwhile; keep one pipeline per device.
    if (pipelines[key]) pipeline=pipelines[key]; else pipelines[key]=pipeline;
    [lock unlock];
    return pipeline;
}

void *nj_metalfx_new(void *layer, int iw, int ih, int ow, int oh) {
    @autoreleasepool {
        if (@available(macOS 13.0, *)) {
            if (!layer || iw <= 0 || ih <= 0 || ow < iw || oh < ih) return NULL;
            NJSpatial *s = [NJSpatial new];
            s.device = ((__bridge CAMetalLayer *)layer).device;
            if (!s.device || ![MTLFXSpatialScalerDescriptor supportsDevice:s.device]) return NULL;
            s.queue = [s.device newCommandQueue];
            s.convert = nj_conversion_pipeline(s.device);
            MTLFXSpatialScalerDescriptor *d = [MTLFXSpatialScalerDescriptor new];
            d.inputWidth=iw; d.inputHeight=ih; d.outputWidth=ow; d.outputHeight=oh;
            d.colorTextureFormat=MTLPixelFormatBGRA8Unorm;
            d.outputTextureFormat=MTLPixelFormatBGRA8Unorm;
            d.colorProcessingMode=MTLFXSpatialScalerColorProcessingModePerceptual;
            s.scaler=[d newSpatialScalerWithDevice:s.device];
            if (!s.scaler || !s.convert || !s.queue) return NULL;
            MTLTextureDescriptor *t=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:d.colorTextureFormat width:iw height:ih mipmapped:NO];
            t.storageMode=MTLStorageModePrivate;
            t.usage=s.scaler.colorTextureUsage|MTLTextureUsageShaderWrite;
            s.rgb=[s.device newTextureWithDescriptor:t];
            // MetalFX writes only private textures; a blit copies the result into an IOSurface image.
            t.width=ow; t.height=oh; t.usage=s.scaler.outputTextureUsage;
            s.scaled=[s.device newTextureWithDescriptor:t];
            CVMetalTextureCacheRef cache=NULL;
            if (CVMetalTextureCacheCreate(NULL,NULL,s.device,NULL,&cache)!=kCVReturnSuccess) return NULL;
            s.cache=cache;
            if (!s.rgb || !s.scaled) return NULL;
            return (__bridge_retained void *)s;
        }
        return NULL;
    }
}

// Returns a +1 CVPixelBuffer from the ring (or, with every ring image busy, a new one). There is
// no CPU pixel readback; the wait keeps SDL's queue from reading the image before it is written.
void *nj_metalfx_draw(void *context, void *input, const float *parameters, double *gpu_ms) {
    @autoreleasepool {
        NJSpatial *s=(__bridge NJSpatial *)context;
        CVPixelBufferRef buffer=input;
        if (!buffer || CVPixelBufferGetPlaneCount(buffer)!=2) return NULL;
        OSType format=CVPixelBufferGetPixelFormatType(buffer);
        bool ten=format==kCVPixelFormatType_420YpCbCr10BiPlanarVideoRange || format==kCVPixelFormatType_420YpCbCr10BiPlanarFullRange;
        if (!ten && format!=kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange && format!=kCVPixelFormatType_420YpCbCr8BiPlanarFullRange) return NULL;
        CVMetalTextureRef planes[2]={NULL,NULL};
        CVPixelBufferRef output=NULL;
        id<MTLTexture> target=nil;
        for (int i=0;i<2;i++) {
            MTLPixelFormat f=i ? (ten ? MTLPixelFormatRG16Unorm : MTLPixelFormatRG8Unorm) : (ten ? MTLPixelFormatR16Unorm : MTLPixelFormatR8Unorm);
            if (CVMetalTextureCacheCreateTextureFromImage(NULL,s.cache,buffer,NULL,f,CVPixelBufferGetWidthOfPlane(buffer,i),CVPixelBufferGetHeightOfPlane(buffer,i),i,&planes[i])!=kCVReturnSuccess) goto cleanup;
        }
        for (int k=0;k<NJ_RING && !output;k++) {
            int i=(s->next+k)%NJ_RING;
            if (!s->ring[i]) {
                if (!nj_new_output(s,&s->ring[i],&s->targets[i])) break;
                s->idle_use[i]=IOSurfaceGetUseCount(CVPixelBufferGetIOSurface(s->ring[i]));
            } else if (CFGetRetainCount(s->ring[i])!=1 || IOSurfaceGetUseCount(CVPixelBufferGetIOSurface(s->ring[i]))!=s->idle_use[i]) {
                continue;
            }
            output=(CVPixelBufferRef)CFRetain(s->ring[i]);
            target=s->targets[i];
            s->next=(i+1)%NJ_RING;
        }
        if (!output) {
            if (!nj_new_output(s,&output,&target)) goto cleanup;
        }
        {
            id<MTLCommandBuffer> command=[s.queue commandBuffer];
            id<MTLComputeCommandEncoder> e=[command computeCommandEncoder];
            [e setComputePipelineState:s.convert];
            [e setTexture:CVMetalTextureGetTexture(planes[0]) atIndex:0];
            [e setTexture:CVMetalTextureGetTexture(planes[1]) atIndex:1];
            [e setTexture:s.rgb atIndex:2];
            [e setBytes:parameters length:8*sizeof(float) atIndex:0];
            [e dispatchThreads:MTLSizeMake(s.rgb.width,s.rgb.height,1) threadsPerThreadgroup:MTLSizeMake(8,8,1)];
            [e endEncoding];
            s.scaler.colorTexture=s.rgb; s.scaler.outputTexture=s.scaled;
            s.scaler.inputContentWidth=s.rgb.width; s.scaler.inputContentHeight=s.rgb.height;
            [s.scaler encodeToCommandBuffer:command];
            id<MTLBlitCommandEncoder> blit=[command blitCommandEncoder];
            [blit copyFromTexture:s.scaled toTexture:target]; [blit endEncoding];
            [command commit]; [command waitUntilCompleted];
            if (command.status==MTLCommandBufferStatusError) { NSLog(@"Nightjar MetalFX: %@",command.error); CFRelease(output); output=NULL; }
            if (gpu_ms) *gpu_ms=(command.GPUEndTime-command.GPUStartTime)*1000;
        }
cleanup:
        for (int i=0;i<2;i++) if (planes[i]) CFRelease(planes[i]);
        CVMetalTextureCacheFlush(s.cache,0);
        return output;
    }
}
void nj_metalfx_free(void *context) { @autoreleasepool { (void)CFBridgingRelease(context); } }
void nj_metalfx_buffer_free(void *buffer) { if (buffer) CFRelease(buffer); }
