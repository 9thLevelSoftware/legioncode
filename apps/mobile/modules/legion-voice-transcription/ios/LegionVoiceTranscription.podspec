Pod::Spec.new do |s|
  s.name = 'LegionVoiceTranscription'
  s.version = '1.0.0'
  s.summary = 'Local recording conversion for LegionCode dictation.'
  s.author = 'LegionCode'
  s.homepage = 'https://legion.codes'
  s.platform = :ios, '18.0'
  s.source = { :path => '.' }
  s.static_framework = true
  s.dependency 'ExpoModulesCore'
  s.source_files = '*.swift'
  s.frameworks = 'AudioToolbox'
end
