# The app's own module: the organization device management pins (MDM AppConfig). Expo autolinks it from
# `modules/`; it has no package.json, so its version is written here.
Pod::Spec.new do |s|
  s.name           = 'ManagedConfig'
  s.version        = '1.0.0'
  s.summary        = 'Managed app configuration from device management'
  s.description    = s.summary
  s.license        = 'UNLICENSED'
  s.author         = 'Nexus'
  s.homepage       = 'https://nexus.xyz'
  s.platforms      = { :ios => '16.4' }
  s.source         = { git: '' }
  s.static_framework = true

  s.dependency 'ExpoModulesCore'

  s.pod_target_xcconfig = {
    'DEFINES_MODULE' => 'YES',
    'SWIFT_COMPILATION_MODE' => 'wholemodule'
  }

  s.source_files = "**/*.swift"
end
