Pod::Spec.new do |s|
  s.name           = 'LegionNativeControls'
  s.version        = '1.0.0'
  s.summary        = 'Native UIKit controls for LegionCode mobile.'
  s.description    = 'UIKit-backed controls that match native iOS navigation chrome.'
  s.author         = 'LegionCode'
  s.homepage       = 'https://legion.codes'
  s.platforms      = {
    :ios => '18.0',
  }
  s.source         = { :path => '.' }
  s.static_framework = true

  s.dependency 'ExpoModulesCore'
  s.pod_target_xcconfig = {
    'DEFINES_MODULE' => 'YES',
  }
  s.resource_bundles = { 'LegionNativeControlIcons' => ['Resources/Icons/pull-request*.png', 'Resources/Icons/issue*.png'] }
  s.source_files = '**/*.{h,m,mm,swift,hpp,cpp}'
end
