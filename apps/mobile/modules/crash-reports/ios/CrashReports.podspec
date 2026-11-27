# The app's own module: native crashes from MetricKit, kept until the app sends them. Expo autolinks it
# from `modules/`; it has no package.json, so its version is written here.
Pod::Spec.new do |s|
  s.name           = 'CrashReports'
  s.version        = '1.0.0'
  s.summary        = 'Native crashes from MetricKit, kept until the app sends them'
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
